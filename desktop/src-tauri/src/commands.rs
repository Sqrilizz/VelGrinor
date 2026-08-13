use crate::discord_rpc::DiscordRpc;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use velgrinor::accounts::{
    create_offline_account, delete_account_tokens, load_accounts, remove_account, save_accounts,
    set_active, upsert_account, Account, Accounts,
};
use velgrinor::auth::{
    begin_browser_login, finish_browser_login, request_device_code, DeviceCode,
    DEFAULT_MS_CLIENT_ID, MS_BROWSER_REDIRECT_URL,
};
use velgrinor::config::{load_config, save_config, Config};
use velgrinor::content_store::{
    ContentItem, ContentStore, ContentType, ContentVersion, Platform, SearchOptions,
};
use velgrinor::crash::{analyze_crash, CrashAnalysis};
use velgrinor::curseforge_pack::import_curseforge_zip_managed;
use velgrinor::dependencies::{resolve_store_install_plan, InstallPlan};
use velgrinor::diagnostics::{apply_fix, diagnose_profile, DiagnosticFix, DiagnosticReport};
use velgrinor::download::{DownloadManager, DownloadRequest, DownloadSnapshot, DownloadStatus};
use velgrinor::instance::materialize_instance;
use velgrinor::java::{
    detect_installations, download_and_install_java_managed, fetch_adoptium_release,
    find_compatible_java, get_managed_java, get_required_java_version, is_java_compatible,
    list_managed_runtimes, validate_java_path, AdoptiumRelease, JavaInstallation, JavaValidation,
};
use velgrinor::library::{
    ImportResult, Library, LibraryContentType, LibraryFilter, LibraryItem, LibraryItemInput,
    LibraryStats, PurgeResult, Tag, UnusedItemsSummary,
};
use velgrinor::logs::{
    list_crash_reports, list_log_files, read_log_file, read_log_tail, LogEntry, LogFile, LogWatcher,
};
use velgrinor::minecraft::{prepare, prepare_with_download_manager, LaunchPlan};
use velgrinor::modpack::{
    backup_profile, export_mrpack, import_mrpack_with_download_manager, repair_profile,
    ProfileRepairReport,
};
use velgrinor::modrinth::ModrinthClient;
use velgrinor::ops::{
    ensure_fresh_account, finish_device_code_flow, finish_microsoft_login_with_minecraft,
    parse_loader, resolve_input, resolve_launch_account,
};
use velgrinor::paths::Paths;
use velgrinor::profile::{
    clone_profile, create_profile, delete_profile, diff_profiles, list_profiles, load_profile,
    remove_mod, remove_resourcepack, remove_shaderpack, rename_profile, save_profile, upsert_mod,
    upsert_resourcepack, upsert_shaderpack, ContentRef, Loader, Profile, Runtime,
};
use velgrinor::session::{LaunchManager, SessionInfo, SessionRecord};
use velgrinor::skin::{
    download_and_cache_cape, download_and_cache_skin, get_active_cape, get_active_skin,
    get_avatar_url, get_body_url, get_cape_url, get_profile as get_mc_profile, get_skin_url,
    hide_cape, reset_skin, set_cape, set_skin_url, upload_skin, MinecraftProfile, SkinVariant,
};
use velgrinor::snapshots::{
    create_snapshot, list_snapshots, prune_automatic_snapshots, restore_snapshot, SnapshotInfo,
};
use velgrinor::store::{store_content, ContentKind};
use velgrinor::template::{init_builtin_templates, list_templates, load_template, Template};
use velgrinor::updates::{
    apply_update, check_all_updates, check_profile_updates, get_storage_stats, set_content_enabled,
    set_content_pinned, StorageStats, UpdateCheckResult,
};

static CUSTOM_CHROME_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn set_custom_chrome_enabled(enabled: bool) {
    CUSTOM_CHROME_ENABLED.store(enabled, Ordering::Relaxed);
}

#[tauri::command]
pub fn custom_chrome_enabled_cmd() -> bool {
    CUSTOM_CHROME_ENABLED.load(Ordering::Relaxed)
}

#[derive(Serialize)]
pub struct DiffResult {
    pub only_a: Vec<String>,
    pub only_b: Vec<String>,
    pub both: Vec<String>,
}

#[derive(Serialize)]
pub struct LaunchPlanDto {
    pub instance_dir: String,
    pub java_exec: String,
    pub jvm_args: Vec<String>,
    pub classpath: String,
    pub main_class: String,
    pub game_args: Vec<String>,
}

#[derive(Clone, Serialize)]
pub struct LaunchEvent {
    pub stage: String,
    pub message: Option<String>,
    pub progress: Option<u8>,
    pub session: Option<SessionInfo>,
    pub exit: Option<i32>,
}

#[derive(Clone, Serialize)]
pub struct StoreInstallProgress {
    pub stage: String,
    pub message: String,
    pub progress: u8,
    pub downloaded: Option<u64>,
    pub total: Option<u64>,
}

#[derive(Deserialize)]
pub struct CreateProfileInput {
    pub id: String,
    pub mc_version: String,
    pub loader_type: Option<String>,
    pub loader_version: Option<String>,
    pub java: Option<String>,
    pub memory: Option<String>,
    pub args: Option<String>,
    pub template: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct AccountInfo {
    pub uuid: String,
    pub username: String,
    pub avatar_url: String,
    pub body_url: String,
    pub skin_url: String,
    pub cape_url: String,
    pub profile: Option<MinecraftProfile>,
}

#[derive(Deserialize)]
pub struct StoreSearchInput {
    pub query: String,
    pub content_type: Option<String>,
    pub game_version: Option<String>,
    pub loader: Option<String>,
    pub platform: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Deserialize)]
pub struct StoreInstallInput {
    pub profile_id: Option<String>,
    pub project_id: String,
    pub platform: String,
    pub version_id: Option<String>,
    pub content_type: Option<String>,
}

fn load_paths() -> Result<Paths, String> {
    let paths = Paths::new().map_err(|e| e.to_string())?;
    paths.ensure().map_err(|e| e.to_string())?;
    Ok(paths)
}

fn snapshot_before_change(paths: &Paths, profile_id: &str, reason: &str) -> Result<(), String> {
    if !paths.is_profile_present(profile_id) {
        return Ok(());
    }
    let config = load_config(paths).map_err(|error| error.to_string())?;
    create_snapshot(paths, profile_id, true, reason).map_err(|error| error.to_string())?;
    prune_automatic_snapshots(paths, profile_id, config.automatic_snapshot_retention)
        .map_err(|error| error.to_string())
}

fn resolve_credentials(
    paths: &Paths,
    client_id: Option<String>,
    client_secret: Option<String>,
) -> Result<(String, Option<String>), String> {
    let config = load_config(paths).map_err(|e| e.to_string())?;
    let id = client_id
        .or(config.msa_client_id)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| "missing Microsoft client id; set it in Settings".to_string())?;
    let secret = client_secret
        .or(config.msa_client_secret)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    Ok((id, secret))
}

fn redact_config_secrets(mut config: Config) -> Config {
    config.msa_client_secret = None;
    config.curseforge_api_key = None;
    config
}

#[tauri::command]
pub fn list_profiles_cmd() -> Result<Vec<String>, String> {
    let paths = load_paths()?;
    list_profiles(&paths).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn load_profile_cmd(id: String) -> Result<Profile, String> {
    let paths = load_paths()?;
    load_profile(&paths, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_profile_cmd(input: CreateProfileInput) -> Result<Profile, String> {
    let paths = load_paths()?;
    let loader = match (input.loader_type, input.loader_version) {
        (Some(loader_type), Some(loader_version)) => {
            let loader_string = format!("{}@{}", loader_type.trim(), loader_version.trim());
            Some(parse_loader(&loader_string).map_err(|e| e.to_string())?)
        }
        (None, None) => None,
        _ => {
            return Err("loader type and version must both be provided".to_string());
        }
    };

    let args = input
        .args
        .unwrap_or_default()
        .split_whitespace()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    let runtime = Runtime {
        java: input.java.filter(|v| !v.trim().is_empty()),
        memory: input.memory.filter(|v| !v.trim().is_empty()),
        args,
    };

    create_profile(&paths, &input.id, &input.mc_version, loader, runtime).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clone_profile_cmd(src: String, dst: String) -> Result<Profile, String> {
    let paths = load_paths()?;
    clone_profile(&paths, &src, &dst).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_profile_cmd(id: String) -> Result<(), String> {
    let paths = load_paths()?;
    delete_profile(&paths, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn rename_profile_cmd(id: String, new_id: String) -> Result<Profile, String> {
    let paths = load_paths()?;
    rename_profile(&paths, &id, &new_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_profile_version_cmd(
    id: String,
    mc_version: String,
    loader_type: Option<String>,
    loader_version: Option<String>,
) -> Result<Profile, String> {
    let paths = load_paths()?;
    snapshot_before_change(&paths, &id, "before version change")?;
    let mut profile = load_profile(&paths, &id).map_err(|e| e.to_string())?;

    // Update MC version
    profile.mc_version = mc_version;

    // Update loader
    profile.loader = match (loader_type, loader_version) {
        (Some(lt), Some(lv)) if !lt.is_empty() && !lv.is_empty() => Some(Loader {
            loader_type: lt,
            version: lv,
        }),
        _ => None,
    };

    save_profile(&paths, &profile).map_err(|e| e.to_string())?;
    Ok(profile)
}

#[tauri::command]
pub fn diff_profiles_cmd(a: String, b: String) -> Result<DiffResult, String> {
    let paths = load_paths()?;
    let profile_a = load_profile(&paths, &a).map_err(|e| e.to_string())?;
    let profile_b = load_profile(&paths, &b).map_err(|e| e.to_string())?;
    let (only_a, only_b, both) = diff_profiles(&profile_a, &profile_b);
    Ok(DiffResult {
        only_a,
        only_b,
        both,
    })
}

fn add_content(
    profile_id: &str,
    input: &str,
    name: Option<String>,
    version: Option<String>,
    kind: ContentKind,
) -> Result<bool, String> {
    let paths = load_paths()?;
    snapshot_before_change(&paths, profile_id, "before adding content")?;
    let mut profile_data = load_profile(&paths, profile_id).map_err(|e| e.to_string())?;
    let (path, source, file_name_hint) = resolve_input(&paths, input).map_err(|e| e.to_string())?;
    let stored = store_content(&paths, kind, &path, source.clone(), file_name_hint.clone())
        .map_err(|e| e.to_string())?;

    // Auto-add to library
    if let Ok(library) = Library::from_paths(&paths) {
        let lib_content_type = match kind {
            ContentKind::Mod => "mod",
            ContentKind::ModPack => "modpack",
            ContentKind::ResourcePack => "resourcepack",
            ContentKind::ShaderPack => "shaderpack",
            ContentKind::Skin => "skin",
        };
        let hash = stored.hash.strip_prefix("sha256:").unwrap_or(&stored.hash);
        let lib_input = LibraryItemInput {
            hash: hash.to_string(),
            content_type: Some(lib_content_type.to_string()),
            name: Some(name.clone().unwrap_or_else(|| stored.name.clone())),
            file_name: file_name_hint.clone(),
            source_url: source.clone(),
            source_platform: if input.contains("modrinth.com") {
                Some("modrinth".to_string())
            } else if input.contains("curseforge.com") {
                Some("curseforge".to_string())
            } else {
                Some("local".to_string())
            },
            ..Default::default()
        };
        if let Ok(lib_item) = library.add_item(&lib_input) {
            let version_tag = format!("mc:{}", profile_data.mc_version);
            let _ = library.add_tag_to_item(lib_item.id, &version_tag);
            if let Some(loader) = profile_data.loader.as_ref() {
                let loader_tag = format!("loader:{}", loader.loader_type);
                let _ = library.add_tag_to_item(lib_item.id, &loader_tag);
            }
        }
    }

    let content_ref = ContentRef {
        name: name.unwrap_or(stored.name),
        hash: stored.hash,
        version,
        source: stored.source,
        file_name: Some(stored.file_name),
        platform: None, // Manual import via UI
        project_id: None,
        version_id: None,
        enabled: true,
        pinned: false,
    };

    let changed = match kind {
        ContentKind::Mod => upsert_mod(&mut profile_data, content_ref),
        ContentKind::ModPack => false,
        ContentKind::ResourcePack => upsert_resourcepack(&mut profile_data, content_ref),
        ContentKind::ShaderPack => upsert_shaderpack(&mut profile_data, content_ref),
        ContentKind::Skin => false, // Skins are not added to profiles
    };
    save_profile(&paths, &profile_data).map_err(|e| e.to_string())?;
    Ok(changed)
}

fn remove_content(profile_id: &str, target: &str, kind: ContentKind) -> Result<bool, String> {
    let paths = load_paths()?;
    snapshot_before_change(&paths, profile_id, "before removing content")?;
    let mut profile_data = load_profile(&paths, profile_id).map_err(|e| e.to_string())?;
    let changed = match kind {
        ContentKind::Mod => remove_mod(&mut profile_data, target),
        ContentKind::ModPack => false,
        ContentKind::ResourcePack => remove_resourcepack(&mut profile_data, target),
        ContentKind::ShaderPack => remove_shaderpack(&mut profile_data, target),
        ContentKind::Skin => false, // Skins are not removed from profiles
    };
    if changed {
        save_profile(&paths, &profile_data).map_err(|e| e.to_string())?;
    }
    Ok(changed)
}

#[tauri::command]
pub fn add_mod_cmd(
    profile_id: String,
    input: String,
    name: Option<String>,
    version: Option<String>,
) -> Result<bool, String> {
    add_content(&profile_id, &input, name, version, ContentKind::Mod)
}

#[tauri::command]
pub fn add_resourcepack_cmd(
    profile_id: String,
    input: String,
    name: Option<String>,
    version: Option<String>,
) -> Result<bool, String> {
    add_content(
        &profile_id,
        &input,
        name,
        version,
        ContentKind::ResourcePack,
    )
}

#[tauri::command]
pub fn add_shaderpack_cmd(
    profile_id: String,
    input: String,
    name: Option<String>,
    version: Option<String>,
) -> Result<bool, String> {
    add_content(&profile_id, &input, name, version, ContentKind::ShaderPack)
}

#[tauri::command]
pub fn remove_mod_cmd(profile_id: String, target: String) -> Result<bool, String> {
    remove_content(&profile_id, &target, ContentKind::Mod)
}

#[tauri::command]
pub fn remove_resourcepack_cmd(profile_id: String, target: String) -> Result<bool, String> {
    remove_content(&profile_id, &target, ContentKind::ResourcePack)
}

#[tauri::command]
pub fn remove_shaderpack_cmd(profile_id: String, target: String) -> Result<bool, String> {
    remove_content(&profile_id, &target, ContentKind::ShaderPack)
}

#[tauri::command]
pub fn list_accounts_cmd() -> Result<Accounts, String> {
    let paths = load_paths()?;
    load_accounts(&paths).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_offline_account_cmd(username: String) -> Result<Account, String> {
    let paths = load_paths()?;
    let account = create_offline_account(&username).map_err(|e| e.to_string())?;
    let mut accounts = load_accounts(&paths).map_err(|e| e.to_string())?;
    accounts.active = Some(account.uuid.clone());
    upsert_account(&mut accounts, account.clone());
    save_accounts(&paths, &accounts).map_err(|e| e.to_string())?;
    Ok(account)
}

#[tauri::command]
pub async fn microsoft_browser_login_cmd(app: AppHandle) -> Result<Account, String> {
    let paths = load_paths()?;
    let flow = tauri::async_runtime::spawn_blocking(begin_browser_login)
        .await
        .map_err(|err| format!("Microsoft sign-in worker failed: {err}"))?
        .map_err(|err| err.to_string())?;

    if let Some(existing) = app.get_webview_window("microsoft-signin") {
        let _ = existing.close();
    }

    let window = WebviewWindowBuilder::new(
        &app,
        "microsoft-signin",
        WebviewUrl::External(
            flow.auth_url
                .parse()
                .map_err(|_| "invalid Microsoft authorization URL")?,
        ),
    )
    .title("Sign in with Microsoft")
    .inner_size(520.0, 720.0)
    .min_inner_size(420.0, 560.0)
    .center()
    .build()
    .map_err(|err| format!("failed to open Microsoft sign-in: {err}"))?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    let (code, state) = loop {
        if std::time::Instant::now() >= deadline {
            let _ = window.close();
            return Err("Microsoft sign-in timed out".to_string());
        }
        let url = window
            .url()
            .map_err(|_| "Microsoft sign-in was cancelled".to_string())?;
        if url.as_str().starts_with(MS_BROWSER_REDIRECT_URL) {
            let params: std::collections::HashMap<String, String> =
                url.query_pairs().into_owned().collect();
            if let Some(error) = params.get("error") {
                let detail = params.get("error_description").unwrap_or(error);
                let _ = window.close();
                return Err(format!("Microsoft sign-in failed: {detail}"));
            }
            let code = params.get("code").cloned().ok_or_else(|| {
                "Microsoft callback did not include an authorization code".to_string()
            })?;
            let state = params
                .get("state")
                .cloned()
                .ok_or_else(|| "Microsoft callback did not include OAuth state".to_string())?;
            break (code, state);
        }

        std::thread::sleep(std::time::Duration::from_millis(100));
    };

    let _ = window.close();
    tauri::async_runtime::spawn_blocking(move || {
        let (token, minecraft) =
            finish_browser_login(&code, &state, &flow).map_err(|err| err.to_string())?;
        finish_microsoft_login_with_minecraft(&paths, token, minecraft, DEFAULT_MS_CLIENT_ID)
            .map_err(|err| err.to_string())
    })
    .await
    .map_err(|err| format!("Microsoft sign-in worker failed: {err}"))?
}

#[tauri::command]
pub fn set_active_account_cmd(id: String) -> Result<(), String> {
    let paths = load_paths()?;
    let mut accounts = load_accounts(&paths).map_err(|e| e.to_string())?;
    if set_active(&mut accounts, &id) {
        save_accounts(&paths, &accounts).map_err(|e| e.to_string())?;
        Ok(())
    } else {
        Err("account not found".to_string())
    }
}

#[tauri::command]
pub fn remove_account_cmd(id: String) -> Result<(), String> {
    let paths = load_paths()?;
    let mut accounts = load_accounts(&paths).map_err(|e| e.to_string())?;
    let removed_uuids = remove_account(&mut accounts, &id);
    if !removed_uuids.is_empty() {
        // Save accounts first, then delete tokens to avoid inconsistent state
        save_accounts(&paths, &accounts).map_err(|e| e.to_string())?;
        for uuid in &removed_uuids {
            delete_account_tokens(&paths, uuid).map_err(|e| e.to_string())?;
        }
        Ok(())
    } else {
        Err("account not found".to_string())
    }
}

#[tauri::command]
pub async fn refresh_account_session_cmd(id: String) -> Result<Account, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let paths = load_paths()?;
        ensure_fresh_account(&paths, Some(id)).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("account refresh task failed: {error}"))?
}

#[tauri::command]
pub fn get_config_cmd() -> Result<Config, String> {
    let paths = load_paths()?;
    load_config(&paths)
        .map(redact_config_secrets)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_config_cmd(
    client_id: Option<String>,
    client_secret: Option<String>,
) -> Result<Config, String> {
    let paths = load_paths()?;
    let mut config = load_config(&paths).map_err(|e| e.to_string())?;
    config.msa_client_id = client_id.filter(|v| !v.trim().is_empty());
    config.msa_client_secret = client_secret.filter(|v| !v.trim().is_empty());
    save_config(&paths, &config).map_err(|e| e.to_string())?;
    Ok(redact_config_secrets(config))
}

#[tauri::command]
pub fn get_curseforge_api_key_status_cmd() -> Result<bool, String> {
    let paths = load_paths()?;
    let config = load_config(&paths).map_err(|e| e.to_string())?;
    Ok(config
        .curseforge_api_key
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty()))
}

#[tauri::command]
pub fn set_curseforge_api_key_cmd(api_key: Option<String>) -> Result<bool, String> {
    let paths = load_paths()?;
    let mut config = load_config(&paths).map_err(|e| e.to_string())?;
    config.curseforge_api_key = api_key
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    save_config(&paths, &config).map_err(|e| e.to_string())?;
    get_curseforge_api_key_status_cmd()
}

#[tauri::command]
pub fn set_discord_rpc_enabled_cmd(
    rpc: State<'_, DiscordRpc>,
    enabled: bool,
) -> Result<Config, String> {
    let paths = load_paths()?;
    let mut config = load_config(&paths).map_err(|e| e.to_string())?;
    config.discord_rpc_enabled = enabled;
    save_config(&paths, &config).map_err(|e| e.to_string())?;
    rpc.configure(enabled, config.discord_app_id.clone());
    Ok(redact_config_secrets(config))
}

#[tauri::command]
pub fn update_discord_rpc_cmd(
    rpc: State<'_, DiscordRpc>,
    profile_id: Option<String>,
) -> Result<(), String> {
    let profile = match profile_id {
        Some(id) => {
            let paths = load_paths()?;
            Some(load_profile(&paths, &id).map_err(|e| e.to_string())?)
        }
        None => None,
    };
    rpc.browsing(
        profile.as_ref().map(|value| value.id.as_str()),
        profile.as_ref().map(|value| value.mc_version.as_str()),
    );
    Ok(())
}

#[tauri::command]
pub fn request_device_code_cmd(
    client_id: Option<String>,
    client_secret: Option<String>,
) -> Result<DeviceCode, String> {
    let paths = load_paths()?;
    let (id, secret) = resolve_credentials(&paths, client_id, client_secret)?;
    request_device_code(&id, secret.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn finish_device_code_flow_cmd(
    client_id: Option<String>,
    client_secret: Option<String>,
    device: DeviceCode,
) -> Result<Account, String> {
    let paths = load_paths()?;
    let (id, secret) = resolve_credentials(&paths, client_id, client_secret)?;
    finish_device_code_flow(&paths, &id, secret.as_deref(), &device).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn prepare_profile_cmd(
    profile_id: String,
    account_id: Option<String>,
) -> Result<LaunchPlanDto, String> {
    let paths = load_paths()?;
    let profile = load_profile(&paths, &profile_id).map_err(|e| e.to_string())?;
    let account = resolve_launch_account(&paths, account_id).map_err(|e| e.to_string())?;
    let plan = prepare(&paths, &profile, &account).map_err(|e| e.to_string())?;
    Ok(LaunchPlanDto::from(plan))
}

#[tauri::command]
pub fn repair_profile_cmd(profile_id: String) -> Result<ProfileRepairReport, String> {
    let paths = load_paths()?;
    repair_profile(&paths, &profile_id).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn export_profile_mrpack_cmd(profile_id: String, output_path: String) -> Result<(), String> {
    let paths = load_paths()?;
    export_mrpack(&paths, &profile_id, std::path::Path::new(&output_path))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn backup_profile_cmd(profile_id: String) -> Result<String, String> {
    let paths = load_paths()?;
    backup_profile(&paths, &profile_id)
        .map(|path| path.to_string_lossy().to_string())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn launch_profile_cmd(
    app: AppHandle,
    rpc: State<'_, DiscordRpc>,
    launch_manager: State<'_, LaunchManager>,
    download_manager: State<'_, DownloadManager>,
    profile_id: String,
    account_id: Option<String>,
    force: Option<bool>,
) -> Result<(), String> {
    let app_handle = app.clone();
    let discord_rpc = rpc.inner().clone();
    let launch_manager = launch_manager.inner().clone();
    let download_manager = download_manager.inner().clone();

    // Emit initial status immediately before spawning thread
    let _ = app.emit(
        "launch-status",
        LaunchEvent {
            stage: "queued".to_string(),
            message: Some("Starting launch...".to_string()),
            progress: Some(0),
            session: None,
            exit: None,
        },
    );

    // Use spawn_blocking for blocking I/O operations (HTTP requests, file I/O)
    tauri::async_runtime::spawn_blocking(move || {
        match run_launch(
            app_handle.clone(),
            discord_rpc.clone(),
            launch_manager,
            download_manager,
            profile_id.clone(),
            account_id,
            force.unwrap_or(false),
        ) {
            Ok(()) => {}
            Err(err) => {
                let _ = app_handle.emit(
                    "launch-status",
                    LaunchEvent {
                        stage: "error".to_string(),
                        message: Some(err),
                        progress: None,
                        session: None,
                        exit: None,
                    },
                );
            }
        }
        if let Ok(paths) = load_paths() {
            if let Ok(profile) = load_profile(&paths, &profile_id) {
                discord_rpc.browsing(Some(&profile.id), Some(&profile.mc_version));
            }
        }
    });
    Ok(())
}

#[tauri::command]
pub fn instance_path_cmd(profile_id: String) -> Result<String, String> {
    let paths = load_paths()?;
    Ok(paths
        .instance_dir(&profile_id)
        .to_string_lossy()
        .to_string())
}

#[tauri::command]
pub fn get_active_session_cmd(launch_manager: State<'_, LaunchManager>) -> Option<SessionInfo> {
    launch_manager.get_active_session()
}

#[tauri::command]
pub fn list_session_records_cmd(
    launch_manager: State<'_, LaunchManager>,
) -> Result<Vec<SessionRecord>, String> {
    launch_manager.records().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn stop_session_cmd(
    launch_manager: State<'_, LaunchManager>,
    session_id: String,
) -> Result<(), String> {
    launch_manager
        .stop(&session_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn diagnose_profile_cmd(profile_id: String) -> Result<DiagnosticReport, String> {
    let paths = load_paths()?;
    let profile = load_profile(&paths, &profile_id).map_err(|error| error.to_string())?;
    diagnose_profile(&paths, &profile).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn apply_diagnostic_fix_cmd(
    profile_id: String,
    fix: DiagnosticFix,
) -> Result<DiagnosticReport, String> {
    let paths = load_paths()?;
    let config = load_config(&paths).map_err(|error| error.to_string())?;
    create_snapshot(&paths, &profile_id, true, "before diagnostic fix")
        .map_err(|error| error.to_string())?;
    prune_automatic_snapshots(&paths, &profile_id, config.automatic_snapshot_retention)
        .map_err(|error| error.to_string())?;
    let mut profile = load_profile(&paths, &profile_id).map_err(|error| error.to_string())?;
    apply_fix(&paths, &mut profile, &fix).map_err(|error| error.to_string())?;
    diagnose_profile(&paths, &profile).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn create_snapshot_cmd(profile_id: String, reason: String) -> Result<SnapshotInfo, String> {
    let paths = load_paths()?;
    create_snapshot(&paths, &profile_id, false, reason).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn list_snapshots_cmd(profile_id: String) -> Result<Vec<SnapshotInfo>, String> {
    let paths = load_paths()?;
    list_snapshots(&paths, &profile_id).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn rollback_snapshot_cmd(
    profile_id: String,
    snapshot_id: String,
) -> Result<SnapshotInfo, String> {
    let paths = load_paths()?;
    restore_snapshot(&paths, &profile_id, &snapshot_id).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn analyze_last_crash_cmd(
    profile_id: String,
    exit_code: Option<i32>,
) -> Result<CrashAnalysis, String> {
    let paths = load_paths()?;
    let crash_report = list_crash_reports(&paths, &profile_id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .max_by_key(|file| file.modified)
        .and_then(|file| std::fs::read_to_string(file.path).ok());
    let log = std::fs::read_to_string(paths.instance_logs_dir(&profile_id).join("latest.log"))
        .unwrap_or_default();
    Ok(analyze_crash(crash_report.as_deref(), &log, exit_code))
}

#[tauri::command]
pub fn disable_suspected_mod_cmd(
    profile_id: String,
    suspected_mod: String,
) -> Result<Profile, String> {
    let paths = load_paths()?;
    snapshot_before_change(&paths, &profile_id, "before disabling crash suspect")?;
    let mut profile = load_profile(&paths, &profile_id).map_err(|error| error.to_string())?;
    let suspect = suspected_mod.to_ascii_lowercase();
    let item = profile
        .mods
        .iter_mut()
        .find(|item| {
            item.file_name
                .as_deref()
                .unwrap_or(&item.name)
                .to_ascii_lowercase()
                == suspect
                || item.name.to_ascii_lowercase() == suspect
        })
        .ok_or_else(|| format!("suspected mod is not installed: {suspected_mod}"))?;
    item.enabled = false;
    save_profile(&paths, &profile).map_err(|error| error.to_string())?;
    Ok(profile)
}

#[tauri::command]
pub fn list_downloads_cmd(download_manager: State<'_, DownloadManager>) -> Vec<DownloadSnapshot> {
    download_manager.snapshots()
}

#[tauri::command]
pub fn pause_download_cmd(
    download_manager: State<'_, DownloadManager>,
    id: String,
) -> Result<(), String> {
    download_manager
        .pause(&id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn resume_download_cmd(
    app: AppHandle,
    download_manager: State<'_, DownloadManager>,
    id: String,
) -> Result<(), String> {
    download_manager
        .resume(&id)
        .map_err(|error| error.to_string())?;
    let manager = download_manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let progress_app = app.clone();
        let _ = manager.run_pending(move |snapshot| {
            let _ = progress_app.emit("download-progress", snapshot);
        });
    });
    Ok(())
}

#[tauri::command]
pub fn cancel_download_cmd(
    download_manager: State<'_, DownloadManager>,
    id: String,
) -> Result<(), String> {
    download_manager
        .cancel(&id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn retry_download_cmd(
    app: AppHandle,
    download_manager: State<'_, DownloadManager>,
    id: String,
) -> Result<(), String> {
    download_manager
        .retry(&id)
        .map_err(|error| error.to_string())?;
    let manager = download_manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let progress_app = app.clone();
        let _ = manager.run_pending(move |snapshot| {
            let _ = progress_app.emit("download-progress", snapshot);
        });
    });
    Ok(())
}

fn run_launch(
    app: AppHandle,
    rpc: DiscordRpc,
    launch_manager: LaunchManager,
    download_manager: DownloadManager,
    profile_id: String,
    account_id: Option<String>,
    force: bool,
) -> Result<(), String> {
    let _ = app.emit(
        "launch-status",
        LaunchEvent {
            stage: "preparing".to_string(),
            message: Some("Initializing".to_string()),
            progress: Some(0),
            session: None,
            exit: None,
        },
    );

    let paths = load_paths()?;
    let profile =
        load_profile(&paths, &profile_id).map_err(|e| format!("Failed to load profile: {}", e))?;
    let mut diagnostics =
        diagnose_profile(&paths, &profile).map_err(|e| format!("Diagnostics failed: {e}"))?;
    if diagnostics
        .issues
        .iter()
        .any(|issue| issue.code == "legacy_symlink")
    {
        materialize_instance(&paths, &profile)
            .map_err(|error| format!("Failed to migrate legacy instance: {error}"))?;
        diagnostics =
            diagnose_profile(&paths, &profile).map_err(|e| format!("Diagnostics failed: {e}"))?;
    }
    if diagnostics.blocking {
        return Err(diagnostics
            .issues
            .into_iter()
            .filter(|issue| issue.severity == velgrinor::diagnostics::DiagnosticSeverity::Error)
            .map(|issue| issue.message)
            .collect::<Vec<_>>()
            .join("; "));
    }
    let warnings = diagnostics
        .issues
        .iter()
        .filter(|issue| issue.severity == velgrinor::diagnostics::DiagnosticSeverity::Warning)
        .map(|issue| issue.message.clone())
        .collect::<Vec<_>>();
    if !warnings.is_empty() && !force {
        return Err(format!(
            "launch_confirmation_required:{}",
            warnings.join("; ")
        ));
    }
    rpc.preparing(&profile.id, &profile.mc_version);
    let account = resolve_launch_account(&paths, account_id)
        .map_err(|e| format!("Failed to resolve account: {}", e))?;
    let progress_app = app.clone();
    let download_app = app.clone();
    let plan = prepare_with_download_manager(
        &paths,
        &profile,
        &account,
        &download_manager,
        &mut |percent, message| {
            let _ = progress_app.emit(
                "launch-status",
                LaunchEvent {
                    stage: "preparing".to_string(),
                    message: Some(message),
                    progress: Some(percent),
                    session: None,
                    exit: None,
                },
            );
        },
        move |snapshot| {
            let _ = download_app.emit("download-progress", snapshot);
        },
    )
    .map_err(|e| format!("Failed to prepare launch: {}", e))?;

    rpc.playing(&profile.id, &profile.mc_version);

    let mut command = Command::new(&plan.java_exec);
    command
        .args(&plan.jvm_args)
        .arg("-cp")
        .arg(&plan.classpath)
        .arg(&plan.main_class)
        .args(&plan.game_args)
        .current_dir(&plan.instance_dir);
    let nvidia_offload = velgrinor::minecraft::configure_gpu_environment(&mut command);

    let _ = app.emit(
        "launch-status",
        LaunchEvent {
            stage: "launching".to_string(),
            message: Some(if nvidia_offload {
                "Starting Minecraft on NVIDIA GPU...".to_string()
            } else {
                "Starting Minecraft...".to_string()
            }),
            progress: Some(100),
            session: None,
            exit: None,
        },
    );

    let finished_app = app.clone();
    let finished_rpc = rpc.clone();
    let finished_profile = profile.clone();
    let finished_manager = launch_manager.clone();
    let finished_paths = paths.clone();
    let restore_window = load_config(&paths)
        .map(|config| config.restore_on_game_exit)
        .unwrap_or(true);
    let java = plan.java_exec.clone();
    let session = launch_manager
        .launch(
            command,
            profile.id.clone(),
            java,
            profile.runtime.memory.clone(),
            nvidia_offload.then(|| "NVIDIA".to_string()),
            move |record| {
                let failed = record.exit_code.unwrap_or(1) != 0;
                if failed {
                    if let Ok(reports) =
                        list_crash_reports(&finished_paths, &record.session.profile_id)
                    {
                        if let Some(report) = reports
                            .into_iter()
                            .filter(|report| report.modified >= record.session.started_at)
                            .max_by_key(|report| report.modified)
                        {
                            let _ = finished_manager
                                .attach_crash_report(&record.session.session_id, report.path);
                        }
                    }
                }
                let _ = finished_app.emit(
                    "launch-status",
                    LaunchEvent {
                        stage: if failed { "error" } else { "done" }.to_string(),
                        message: failed.then(|| {
                            format!(
                                "Minecraft exited with code {}",
                                record.exit_code.unwrap_or(-1)
                            )
                        }),
                        progress: Some(100),
                        session: Some(record.session.clone()),
                        exit: record.exit_code,
                    },
                );
                if restore_window {
                    if let Some(window) = finished_app.get_webview_window("main") {
                        let _ = window.unminimize();
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                finished_rpc.browsing(
                    Some(&finished_profile.id),
                    Some(&finished_profile.mc_version),
                );
            },
        )
        .map_err(|e| format!("Failed to start Java: {e}"))?;

    let _ = app.emit(
        "launch-status",
        LaunchEvent {
            stage: "running".to_string(),
            message: Some("Minecraft is running".to_string()),
            progress: Some(100),
            session: Some(session),
            exit: None,
        },
    );

    if load_config(&paths)
        .map(|config| config.minimize_on_game_start)
        .unwrap_or(true)
    {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.minimize();
        }
    }

    Ok(())
}

impl From<LaunchPlan> for LaunchPlanDto {
    fn from(plan: LaunchPlan) -> Self {
        Self {
            instance_dir: plan.instance_dir.to_string_lossy().to_string(),
            java_exec: plan.java_exec,
            jvm_args: plan.jvm_args,
            classpath: plan.classpath,
            main_class: plan.main_class,
            game_args: plan.game_args,
        }
    }
}

// ==================== Account Info / Skin / Cape Commands ====================

#[tauri::command]
pub fn get_account_info_cmd(id: Option<String>) -> Result<AccountInfo, String> {
    let paths = load_paths()?;

    // Ensure tokens are fresh before fetching profile
    let account = ensure_fresh_account(&paths, id.clone()).map_err(|e| e.to_string())?;

    let profile = if account.is_offline() {
        None
    } else {
        get_mc_profile(&account.minecraft.access_token).ok()
    };

    // Get the skin URL from the profile, or fallback to mc-heads.net
    let raw_skin_url = if let Some(ref profile) = profile {
        get_active_skin(profile)
            .map(|skin| skin.url.clone())
            .unwrap_or_else(|| get_skin_url(&account.uuid))
    } else {
        get_skin_url(&account.uuid)
    };

    // Get the cape URL from the profile
    let raw_cape_url = if let Some(ref profile) = profile {
        get_active_cape(profile).map(|cape| cape.url.clone())
    } else {
        None
    };

    // Download and cache the skin to local store, return asset:// URL
    let skin_url = match download_and_cache_skin(&raw_skin_url, &paths.store_skins) {
        Ok(cached_path) => {
            // Return as asset:// URL for Tauri to serve
            format!(
                "asset://localhost/{}",
                cached_path.to_string_lossy().replace('\\', "/")
            )
        }
        Err(_) => {
            // Fallback to mc-heads.net which has CORS support
            get_skin_url(&account.uuid)
        }
    };

    // Download and cache the cape if available
    let cape_url = if let Some(ref url) = raw_cape_url {
        match download_and_cache_cape(url, &paths.store_skins) {
            Ok(Some(cached_path)) => {
                format!(
                    "asset://localhost/{}",
                    cached_path.to_string_lossy().replace('\\', "/")
                )
            }
            _ => get_cape_url(&account.uuid),
        }
    } else {
        get_cape_url(&account.uuid)
    };

    Ok(AccountInfo {
        uuid: account.uuid.clone(),
        username: account.username.clone(),
        avatar_url: get_avatar_url(&account.uuid, 128),
        body_url: get_body_url(&account.uuid, 256),
        skin_url,
        cape_url,
        profile,
    })
}

#[tauri::command]
pub fn upload_skin_cmd(
    id: Option<String>,
    path: String,
    variant: String,
    save_to_library: Option<bool>,
) -> Result<Option<LibraryItem>, String> {
    let paths = load_paths()?;
    let accounts = load_accounts(&paths).map_err(|e| e.to_string())?;

    let target = id
        .or_else(|| accounts.active.clone())
        .ok_or_else(|| "no account selected".to_string())?;

    let account = accounts
        .accounts
        .iter()
        .find(|a| a.uuid == target || a.username.to_lowercase() == target.to_lowercase())
        .ok_or_else(|| "account not found".to_string())?;
    if account.is_offline() {
        return Err("skin changes are unavailable for offline accounts".to_string());
    }

    let skin_path = PathBuf::from(&path);
    let variant: SkinVariant = variant.parse().map_err(|e| format!("{}", e))?;
    upload_skin(&account.minecraft.access_token, &skin_path, variant).map_err(|e| e.to_string())?;

    // Optionally save to library
    if save_to_library.unwrap_or(true) {
        let library = Library::from_paths(&paths).map_err(|e| e.to_string())?;
        let item = library
            .import_file(&paths, &skin_path, LibraryContentType::Skin)
            .map_err(|e| e.to_string())?;
        Ok(Some(item))
    } else {
        Ok(None)
    }
}

#[tauri::command]
pub fn set_skin_url_cmd(id: Option<String>, url: String, variant: String) -> Result<(), String> {
    let paths = load_paths()?;
    let accounts = load_accounts(&paths).map_err(|e| e.to_string())?;

    let target = id
        .or_else(|| accounts.active.clone())
        .ok_or_else(|| "no account selected".to_string())?;

    let account = accounts
        .accounts
        .iter()
        .find(|a| a.uuid == target || a.username.to_lowercase() == target.to_lowercase())
        .ok_or_else(|| "account not found".to_string())?;
    if account.is_offline() {
        return Err("skin changes are unavailable for offline accounts".to_string());
    }

    let variant: SkinVariant = variant.parse().map_err(|e| format!("{}", e))?;
    set_skin_url(&account.minecraft.access_token, &url, variant).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn reset_skin_cmd(id: Option<String>) -> Result<(), String> {
    let paths = load_paths()?;
    let accounts = load_accounts(&paths).map_err(|e| e.to_string())?;

    let target = id
        .or_else(|| accounts.active.clone())
        .ok_or_else(|| "no account selected".to_string())?;

    let account = accounts
        .accounts
        .iter()
        .find(|a| a.uuid == target || a.username.to_lowercase() == target.to_lowercase())
        .ok_or_else(|| "account not found".to_string())?;
    if account.is_offline() {
        return Err("skin changes are unavailable for offline accounts".to_string());
    }

    reset_skin(&account.minecraft.access_token).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn apply_library_skin_cmd(
    id: Option<String>,
    item_id: i64,
    variant: String,
) -> Result<(), String> {
    let paths = load_paths()?;
    let accounts = load_accounts(&paths).map_err(|e| e.to_string())?;

    let target = id
        .or_else(|| accounts.active.clone())
        .ok_or_else(|| "no account selected".to_string())?;

    let account = accounts
        .accounts
        .iter()
        .find(|a| a.uuid == target || a.username.to_lowercase() == target.to_lowercase())
        .ok_or_else(|| "account not found".to_string())?;
    if account.is_offline() {
        return Err("skin changes are unavailable for offline accounts".to_string());
    }

    let library = Library::from_paths(&paths).map_err(|e| e.to_string())?;
    let item = library
        .get_item(item_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "skin not found in library".to_string())?;

    if item.content_type != LibraryContentType::Skin {
        return Err("item is not a skin".to_string());
    }

    let skin_path = paths.store_skin_path(&item.hash);
    if !skin_path.exists() {
        return Err("skin file not found in store".to_string());
    }

    let variant: SkinVariant = variant.parse().map_err(|e| format!("{}", e))?;
    upload_skin(&account.minecraft.access_token, &skin_path, variant).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_cape_cmd(id: Option<String>, cape_id: String) -> Result<(), String> {
    let paths = load_paths()?;
    let accounts = load_accounts(&paths).map_err(|e| e.to_string())?;

    let target = id
        .or_else(|| accounts.active.clone())
        .ok_or_else(|| "no account selected".to_string())?;

    let account = accounts
        .accounts
        .iter()
        .find(|a| a.uuid == target || a.username.to_lowercase() == target.to_lowercase())
        .ok_or_else(|| "account not found".to_string())?;
    if account.is_offline() {
        return Err("cape changes are unavailable for offline accounts".to_string());
    }

    set_cape(&account.minecraft.access_token, &cape_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn hide_cape_cmd(id: Option<String>) -> Result<(), String> {
    let paths = load_paths()?;
    let accounts = load_accounts(&paths).map_err(|e| e.to_string())?;

    let target = id
        .or_else(|| accounts.active.clone())
        .ok_or_else(|| "no account selected".to_string())?;

    let account = accounts
        .accounts
        .iter()
        .find(|a| a.uuid == target || a.username.to_lowercase() == target.to_lowercase())
        .ok_or_else(|| "account not found".to_string())?;
    if account.is_offline() {
        return Err("cape changes are unavailable for offline accounts".to_string());
    }

    hide_cape(&account.minecraft.access_token).map_err(|e| e.to_string())
}

// ==================== Template Commands ====================

#[tauri::command]
pub fn list_templates_cmd() -> Result<Vec<String>, String> {
    let paths = load_paths()?;
    init_builtin_templates(&paths).map_err(|e| e.to_string())?;
    list_templates(&paths).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn load_template_cmd(id: String) -> Result<Template, String> {
    let paths = load_paths()?;
    init_builtin_templates(&paths).map_err(|e| e.to_string())?;
    load_template(&paths, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_profile_from_template_cmd(input: CreateProfileInput) -> Result<Profile, String> {
    let paths = load_paths()?;

    if let Some(template_id) = input.template {
        init_builtin_templates(&paths).map_err(|e| e.to_string())?;
        let template = load_template(&paths, &template_id).map_err(|e| e.to_string())?;

        let loader = template.loader.map(|l| Loader {
            loader_type: l.loader_type,
            version: l.version,
        });

        let runtime = Runtime {
            java: input.java.or(template.runtime.java),
            memory: input.memory.or(template.runtime.memory),
            args: if input
                .args
                .as_ref()
                .map(|a| !a.trim().is_empty())
                .unwrap_or(false)
            {
                input
                    .args
                    .unwrap()
                    .split_whitespace()
                    .map(String::from)
                    .collect()
            } else {
                template.runtime.args
            },
        };

        let mut profile = create_profile(
            &paths,
            &input.id,
            &template.mc_version,
            loader.clone(),
            runtime,
        )
        .map_err(|e| e.to_string())?;

        // Download content from template (mods, shaderpacks, resourcepacks)
        let store = ContentStore::modrinth_only();
        let loader_type = loader.as_ref().map(|l| l.loader_type.as_str());

        for mod_content in &template.mods {
            if !mod_content.required {
                continue;
            }
            if let velgrinor::template::ContentSource::Modrinth { project } = &mod_content.source {
                if let Ok(version) = store.get_latest_version(
                    Platform::Modrinth,
                    project,
                    Some(&template.mc_version),
                    loader_type,
                ) {
                    if let Ok(content_ref) =
                        store.download_to_store(&paths, &version, ContentType::Mod)
                    {
                        upsert_mod(&mut profile, content_ref);
                    }
                }
            }
        }

        for shader in &template.shaderpacks {
            if !shader.required {
                continue;
            }
            if let velgrinor::template::ContentSource::Modrinth { project } = &shader.source {
                if let Ok(version) =
                    store.get_latest_version(Platform::Modrinth, project, None, None)
                {
                    if let Ok(content_ref) =
                        store.download_to_store(&paths, &version, ContentType::ShaderPack)
                    {
                        upsert_shaderpack(&mut profile, content_ref);
                    }
                }
            }
        }

        for pack in &template.resourcepacks {
            if !pack.required {
                continue;
            }
            if let velgrinor::template::ContentSource::Modrinth { project } = &pack.source {
                if let Ok(version) =
                    store.get_latest_version(Platform::Modrinth, project, None, None)
                {
                    if let Ok(content_ref) =
                        store.download_to_store(&paths, &version, ContentType::ResourcePack)
                    {
                        upsert_resourcepack(&mut profile, content_ref);
                    }
                }
            }
        }

        save_profile(&paths, &profile).map_err(|e| e.to_string())?;
        Ok(profile)
    } else {
        // No template, create regular profile
        let loader = match (input.loader_type, input.loader_version) {
            (Some(loader_type), Some(loader_version)) => {
                let loader_string = format!("{}@{}", loader_type.trim(), loader_version.trim());
                Some(parse_loader(&loader_string).map_err(|e| e.to_string())?)
            }
            (None, None) => None,
            _ => return Err("loader type and version must both be provided".to_string()),
        };

        let args = input
            .args
            .unwrap_or_default()
            .split_whitespace()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let runtime = Runtime {
            java: input.java.filter(|v| !v.trim().is_empty()),
            memory: input.memory.filter(|v| !v.trim().is_empty()),
            args,
        };

        create_profile(&paths, &input.id, &input.mc_version, loader, runtime)
            .map_err(|e| e.to_string())
    }
}

// ==================== Content Store Commands ====================

fn parse_platform(s: &str) -> Result<Platform, String> {
    match s.to_lowercase().as_str() {
        "modrinth" => Ok(Platform::Modrinth),
        "curseforge" => Ok(Platform::CurseForge),
        _ => Err(format!("invalid platform: {}", s)),
    }
}

fn parse_content_type(s: &str) -> Result<ContentType, String> {
    match s.to_lowercase().as_str() {
        "mod" => Ok(ContentType::Mod),
        "resourcepack" => Ok(ContentType::ResourcePack),
        "shader" | "shaderpack" => Ok(ContentType::ShaderPack),
        "modpack" => Ok(ContentType::ModPack),
        _ => Err(format!("invalid content type: {}", s)),
    }
}

#[tauri::command]
pub fn store_search_cmd(input: StoreSearchInput) -> Result<Vec<ContentItem>, String> {
    let paths = load_paths()?;
    let config = load_config(&paths).map_err(|e| e.to_string())?;
    let has_cf_key = config.curseforge_api_key.is_some();
    let store = ContentStore::new(config.curseforge_api_key.as_deref());

    let content_type = input
        .content_type
        .as_ref()
        .map(|s| parse_content_type(s))
        .transpose()?;

    let options = SearchOptions {
        query: input.query,
        content_type,
        game_version: input.game_version,
        loader: input.loader,
        limit: input.limit.unwrap_or(20),
        offset: 0,
    };

    match input.platform.as_deref() {
        Some("modrinth") => store.search_modrinth(&options).map_err(|e| e.to_string()),
        Some("curseforge") => {
            if !has_cf_key {
                return Err(
                    "CurseForge search requires an API key. Add it in Settings.".to_string()
                );
            }
            store
                .search_curseforge_only(&options)
                .map_err(|e| e.to_string())
        }
        _ => store.search(&options).map_err(|e| e.to_string()),
    }
}

#[tauri::command]
pub fn store_get_project_cmd(project_id: String, platform: String) -> Result<ContentItem, String> {
    let paths = load_paths()?;
    let config = load_config(&paths).map_err(|e| e.to_string())?;
    let store = ContentStore::new(config.curseforge_api_key.as_deref());
    let platform = parse_platform(&platform)?;
    store
        .get_project(platform, &project_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn store_get_project_icons_cmd(
    project_ids: Vec<String>,
) -> Result<std::collections::HashMap<String, String>, String> {
    let projects = ModrinthClient::new()
        .get_projects(&project_ids)
        .map_err(|e| e.to_string())?;
    Ok(projects
        .into_iter()
        .filter_map(|project| project.icon_url.map(|icon| (project.id, icon)))
        .collect())
}

#[tauri::command]
pub fn store_get_versions_cmd(
    project_id: String,
    platform: String,
    game_version: Option<String>,
    loader: Option<String>,
    profile_id: Option<String>,
) -> Result<Vec<ContentVersion>, String> {
    let paths = load_paths()?;
    let config = load_config(&paths).map_err(|e| e.to_string())?;
    let store = ContentStore::new(config.curseforge_api_key.as_deref());
    let platform = parse_platform(&platform)?;

    // Fetch project to determine content type
    let project = store
        .get_project(platform, &project_id)
        .map_err(|e| e.to_string())?;

    // Determine the effective loader based on content type
    let effective_loader: Option<String> = match project.content_type {
        ContentType::Mod => loader,
        ContentType::ModPack => None,
        ContentType::ShaderPack => {
            // For shaders, detect if the profile has iris/optifine installed
            if let Some(pid) = &profile_id {
                if let Ok(profile) = load_profile(&paths, pid) {
                    profile
                        .primary_shader_loader()
                        .map(|sl| sl.modrinth_name().to_string())
                } else {
                    None
                }
            } else {
                None
            }
        }
        ContentType::ResourcePack => None, // Resourcepacks use "minecraft" loader, no filter needed
    };

    store
        .get_versions(
            platform,
            &project_id,
            game_version.as_deref(),
            effective_loader.as_deref(),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn store_install_plan_cmd(input: StoreInstallInput) -> Result<InstallPlan, String> {
    let paths = load_paths()?;
    let config = load_config(&paths).map_err(|error| error.to_string())?;
    let store = ContentStore::new(config.curseforge_api_key.as_deref());
    let platform = parse_platform(&input.platform)?;
    let profile_id = input
        .profile_id
        .as_deref()
        .ok_or("select a profile first")?;
    let profile = load_profile(&paths, profile_id).map_err(|error| error.to_string())?;
    let loader = profile
        .loader
        .as_ref()
        .map(|value| value.loader_type.as_str());
    let versions = store
        .get_versions(
            platform,
            &input.project_id,
            Some(&profile.mc_version),
            loader,
        )
        .map_err(|error| error.to_string())?;
    let root = if let Some(version_id) = input.version_id.as_deref() {
        versions
            .into_iter()
            .find(|version| version.id == version_id || version.version == version_id)
            .ok_or("version not found")?
    } else {
        versions
            .into_iter()
            .next()
            .ok_or("no compatible version found")?
    };
    let installed = profile
        .mods
        .iter()
        .filter_map(|item| {
            item.project_id
                .clone()
                .map(|project| (project, item.version_id.clone()))
        })
        .collect();
    resolve_store_install_plan(
        &store,
        platform,
        root,
        &profile.mc_version,
        loader,
        &installed,
    )
    .map(|resolved| resolved.plan)
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn store_install_cmd(
    app: AppHandle,
    download_manager: State<'_, DownloadManager>,
    input: StoreInstallInput,
) -> Result<Profile, String> {
    let download_manager = download_manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        install_store_content(&app, &download_manager, input)
    })
    .await
    .map_err(|e| format!("store install task failed: {e}"))?
}

fn emit_store_progress(
    app: &AppHandle,
    stage: &str,
    message: impl Into<String>,
    progress: u8,
    downloaded: Option<u64>,
    total: Option<u64>,
) {
    let _ = app.emit(
        "store-install-progress",
        StoreInstallProgress {
            stage: stage.to_string(),
            message: message.into(),
            progress,
            downloaded,
            total,
        },
    );
}

fn managed_store_download(
    app: &AppHandle,
    manager: &DownloadManager,
    paths: &Paths,
    version: &ContentVersion,
) -> Result<PathBuf, String> {
    managed_store_downloads(app, manager, paths, std::slice::from_ref(version))
        .map(|mut paths| paths.remove(0))
}

fn managed_store_downloads(
    app: &AppHandle,
    manager: &DownloadManager,
    paths: &Paths,
    versions: &[ContentVersion],
) -> Result<Vec<PathBuf>, String> {
    let mut queued = Vec::new();
    for version in versions {
        let destination = paths.cache_downloads.join(format!(
            "{}-{}",
            version.id,
            velgrinor::util::sanitize_filename(&version.filename)
        ));
        let mut request = DownloadRequest::new(&version.download_url, &destination);
        request.expected_size = (version.size > 0).then_some(version.size);
        request.sha1 = version.sha1.clone();
        request.sha256 = version.sha256.clone();
        request.label = Some(version.filename.clone());
        request.group = Some(format!("store:{}", version.project_id));
        let id = manager
            .enqueue(request)
            .map_err(|error| error.to_string())?;
        queued.push((id, destination));
    }
    let progress_app = app.clone();
    manager
        .run_pending(move |snapshot| {
            let _ = progress_app.emit("download-progress", snapshot);
        })
        .map_err(|error| error.to_string())?;
    let snapshots = manager.snapshots();
    for (id, _) in &queued {
        let snapshot = snapshots
            .iter()
            .find(|snapshot| snapshot.request.id == *id)
            .ok_or("download disappeared from queue")?;
        if snapshot.status != DownloadStatus::Completed {
            return Err(snapshot
                .error
                .clone()
                .unwrap_or_else(|| "download did not complete".to_string()));
        }
    }
    Ok(queued
        .into_iter()
        .map(|(_, destination)| destination)
        .collect())
}

fn install_store_content(
    app: &AppHandle,
    download_manager: &DownloadManager,
    input: StoreInstallInput,
) -> Result<Profile, String> {
    emit_store_progress(app, "resolving", "Resolving project", 2, None, None);
    let paths = load_paths()?;
    let config = load_config(&paths).map_err(|e| e.to_string())?;
    let store = ContentStore::new(config.curseforge_api_key.as_deref());

    let platform = parse_platform(&input.platform)?;

    // Get project info to determine content type
    let item = store
        .get_project(platform, &input.project_id)
        .map_err(|e| e.to_string())?;
    let ct = input
        .content_type
        .as_ref()
        .map(|s| parse_content_type(s))
        .transpose()?
        .unwrap_or(item.content_type);

    if ct == ContentType::ModPack {
        emit_store_progress(app, "resolving", "Selecting modpack version", 5, None, None);
        let version = if let Some(v_id) = input.version_id {
            let versions = store
                .get_versions(platform, &input.project_id, None, None)
                .map_err(|e| e.to_string())?;
            versions
                .into_iter()
                .find(|version| version.version == v_id || version.id == v_id)
                .ok_or_else(|| "version not found".to_string())?
        } else {
            store
                .get_latest_version(platform, &input.project_id, None, None)
                .map_err(|e| e.to_string())?
        };
        let downloaded_archive = managed_store_download(app, download_manager, &paths, &version)?;
        let file_name = version.filename.clone();
        emit_store_progress(app, "importing", "Saving modpack archive", 29, None, None);
        let stored = store_content(
            &paths,
            ContentKind::ModPack,
            &downloaded_archive,
            Some(version.download_url.clone()),
            Some(file_name),
        )
        .map_err(|e| format!("failed to store modpack: {e}"))?;
        let archive =
            paths.store_modpack_path(stored.hash.strip_prefix("sha256:").unwrap_or(&stored.hash));
        let import_app = app.clone();
        let profile = if platform == Platform::CurseForge {
            let api_key = config
                .curseforge_api_key
                .as_deref()
                .ok_or("CurseForge API key is required")?;
            let download_app = app.clone();
            import_curseforge_zip_managed(
                &paths,
                &archive,
                None,
                api_key,
                download_manager,
                move |percent, message| {
                    emit_store_progress(&import_app, "importing", message, percent, None, None);
                },
                move |snapshot| {
                    let _ = download_app.emit("download-progress", snapshot);
                },
            )
        } else {
            let download_app = app.clone();
            import_mrpack_with_download_manager(
                &paths,
                &archive,
                None,
                download_manager,
                &mut move |percent, message| {
                    emit_store_progress(&import_app, "importing", message, percent, None, None);
                },
                move |snapshot| {
                    let _ = download_app.emit("download-progress", snapshot);
                },
            )
        }
        .map_err(|e| format!("failed to import modpack: {e}"))?;
        if let Ok(library) = Library::from_paths(&paths) {
            if let Ok(item) = library.add_item(&LibraryItemInput {
                hash: stored.hash,
                content_type: Some("modpack".to_string()),
                name: Some(item.name),
                file_name: Some(stored.file_name),
                file_size: std::fs::metadata(&archive)
                    .ok()
                    .map(|meta| meta.len() as i64),
                source_url: Some(version.download_url),
                source_platform: Some(input.platform.clone()),
                source_project_id: Some(input.project_id),
                source_version: Some(version.version),
                notes: None,
            }) {
                let _ =
                    library.link_item_to_profile(item.id, &profile.id, LibraryContentType::ModPack);
            }
        }
        emit_store_progress(app, "done", "Modpack installed", 100, None, None);
        return Ok(profile);
    }

    let profile_id = input
        .profile_id
        .as_deref()
        .ok_or_else(|| "select a profile before installing content".to_string())?;
    let mut profile = load_profile(&paths, profile_id).map_err(|e| e.to_string())?;

    // Determine effective loader based on content type
    let effective_loader: Option<String> = match ct {
        ContentType::Mod => profile.loader.as_ref().map(|l| l.loader_type.clone()),
        ContentType::ModPack => unreachable!(),
        ContentType::ShaderPack => {
            // For shaders, detect if the profile has iris/optifine installed
            profile
                .primary_shader_loader()
                .map(|sl| sl.modrinth_name().to_string())
        }
        ContentType::ResourcePack => None, // Resourcepacks use "minecraft" loader, no filter needed
    };

    let version = if let Some(v_id) = input.version_id.clone() {
        let versions = store
            .get_versions(platform, &input.project_id, None, None)
            .map_err(|e| e.to_string())?;
        versions
            .into_iter()
            .find(|v| v.version == v_id || v.id == v_id)
            .ok_or_else(|| "version not found".to_string())?
    } else {
        store
            .get_latest_version(
                platform,
                &input.project_id,
                Some(&profile.mc_version),
                effective_loader.as_deref(),
            )
            .map_err(|e| e.to_string())?
    };

    let versions = if ct == ContentType::Mod {
        let installed = profile
            .mods
            .iter()
            .filter_map(|item| {
                item.project_id
                    .clone()
                    .map(|project| (project, item.version_id.clone()))
            })
            .collect();
        resolve_store_install_plan(
            &store,
            platform,
            version,
            &profile.mc_version,
            effective_loader.as_deref(),
            &installed,
        )
        .map_err(|error| error.to_string())?
        .versions
    } else {
        vec![version]
    };
    snapshot_before_change(&paths, profile_id, "before content installation")?;
    let downloaded_versions = managed_store_downloads(app, download_manager, &paths, &versions)?;
    for (index, (install_version, downloaded)) in
        versions.iter().zip(downloaded_versions).enumerate()
    {
        emit_store_progress(
            app,
            "downloading",
            format!("Downloading content {}/{}", index + 1, versions.len()),
            30 + ((index * 60) / versions.len().max(1)) as u8,
            None,
            None,
        );
        let kind = match ct {
            ContentType::Mod => ContentKind::Mod,
            ContentType::ResourcePack => ContentKind::ResourcePack,
            ContentType::ShaderPack => ContentKind::ShaderPack,
            ContentType::ModPack => unreachable!(),
        };
        let stored = store_content(
            &paths,
            kind,
            &downloaded,
            Some(install_version.download_url.clone()),
            Some(install_version.filename.clone()),
        )
        .map_err(|error| error.to_string())?;
        let mut content_ref = ContentRef {
            name: install_version.name.clone(),
            hash: format!("sha256:{}", stored.hash),
            version: Some(install_version.version.clone()),
            source: Some(install_version.download_url.clone()),
            file_name: Some(stored.file_name),
            platform: None,
            project_id: None,
            version_id: None,
            enabled: true,
            pinned: false,
        };
        content_ref.platform = Some(input.platform.clone());
        content_ref.project_id = Some(install_version.project_id.clone());
        content_ref.version_id = Some(install_version.id.clone());
        content_ref.pinned = false;
        if let Ok(project) = store.get_project(platform, &install_version.project_id) {
            content_ref.name = project.name;
        }
        if let Ok(library) = Library::from_paths(&paths) {
            let lib_content_type = match ct {
                ContentType::Mod => "mod",
                ContentType::ResourcePack => "resourcepack",
                ContentType::ShaderPack => "shaderpack",
                ContentType::ModPack => unreachable!(),
            };
            let hash = content_ref
                .hash
                .strip_prefix("sha256:")
                .unwrap_or(&content_ref.hash);
            let _ = library.add_item(&LibraryItemInput {
                hash: hash.to_string(),
                content_type: Some(lib_content_type.to_string()),
                name: Some(content_ref.name.clone()),
                file_name: content_ref.file_name.clone(),
                source_url: content_ref.source.clone(),
                source_platform: Some(input.platform.clone()),
                source_project_id: Some(install_version.project_id.clone()),
                source_version: Some(install_version.version.clone()),
                ..Default::default()
            });
        }
        match ct {
            ContentType::Mod => upsert_mod(&mut profile, content_ref),
            ContentType::ResourcePack => upsert_resourcepack(&mut profile, content_ref),
            ContentType::ShaderPack => upsert_shaderpack(&mut profile, content_ref),
            ContentType::ModPack => unreachable!(),
        };
    }

    save_profile(&paths, &profile).map_err(|e| e.to_string())?;
    emit_store_progress(app, "done", "Content installed", 100, None, None);
    Ok(profile)
}

// ==================== Logs Commands ====================

#[tauri::command]
pub fn list_log_files_cmd(profile_id: String) -> Result<Vec<LogFile>, String> {
    let paths = load_paths()?;
    list_log_files(&paths, &profile_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn read_logs_cmd(
    profile_id: String,
    file: Option<String>,
    lines: Option<usize>,
) -> Result<Vec<LogEntry>, String> {
    let paths = load_paths()?;
    let log_path = if let Some(filename) = file {
        paths.instance_logs_dir(&profile_id).join(filename)
    } else {
        paths.instance_latest_log(&profile_id)
    };

    if !log_path.exists() {
        return Ok(Vec::new());
    }

    if let Some(n) = lines {
        read_log_tail(&log_path, n).map_err(|e| e.to_string())
    } else {
        read_log_file(&log_path).map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub fn list_crash_reports_cmd(profile_id: String) -> Result<Vec<LogFile>, String> {
    let paths = load_paths()?;
    list_crash_reports(&paths, &profile_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn read_crash_report_cmd(profile_id: String, file: Option<String>) -> Result<String, String> {
    let paths = load_paths()?;
    let crash_dir = paths.instance_crash_reports(&profile_id);

    let crash_path = if let Some(filename) = file {
        crash_dir.join(filename)
    } else {
        let files = list_crash_reports(&paths, &profile_id).map_err(|e| e.to_string())?;
        files
            .into_iter()
            .next()
            .map(|f| f.path)
            .ok_or_else(|| "no crash reports found".to_string())?
    };

    if !crash_path.exists() {
        return Err("crash report not found".to_string());
    }

    std::fs::read_to_string(&crash_path).map_err(|e| e.to_string())
}

fn sanitize_event_segment(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '/' | ':' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Start watching a log file and emit events for new entries
#[tauri::command]
pub async fn start_log_watch(app: AppHandle, profile_id: String) -> Result<(), String> {
    let paths = load_paths()?;
    let log_path = paths.instance_latest_log(&profile_id);

    // Spawn background task to watch the log
    std::thread::spawn(move || {
        let mut watcher = LogWatcher::from_start(log_path.clone());
        let event_name = format!("log-entries-{}", sanitize_event_segment(&profile_id));

        loop {
            // Read new entries
            match watcher.read_new() {
                Ok(entries) if !entries.is_empty() => {
                    // Emit event with new log entries
                    if app.emit(&event_name, &entries).is_err() {
                        break; // Window closed
                    }
                }
                Ok(_) => {
                    // No new entries
                }
                Err(_) => {
                    // Error reading log
                }
            }

            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    });

    Ok(())
}

// ============================================================================
// Version fetching commands
// ============================================================================

#[derive(Clone, Serialize, Deserialize)]
pub struct ManifestVersion {
    pub id: String,
    #[serde(rename = "type")]
    pub version_type: String,
    #[serde(rename = "releaseTime")]
    pub release_time: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct VersionManifestResponse {
    versions: Vec<ManifestVersion>,
    latest: Option<LatestVersions>,
}

#[derive(Clone, Serialize, Deserialize)]
struct LatestVersions {
    release: Option<String>,
    snapshot: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct MinecraftVersionsResponse {
    pub versions: Vec<ManifestVersion>,
    pub latest_release: Option<String>,
    pub latest_snapshot: Option<String>,
}

#[tauri::command]
pub fn fetch_minecraft_versions_cmd() -> Result<MinecraftVersionsResponse, String> {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get("https://piston-meta.mojang.com/mc/game/version_manifest_v2.json")
        .send()
        .map_err(|e| format!("Failed to fetch Minecraft versions: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP error: {}", resp.status()));
    }

    let manifest: VersionManifestResponse = resp
        .json()
        .map_err(|e| format!("Failed to parse version manifest: {}", e))?;

    Ok(MinecraftVersionsResponse {
        versions: manifest.versions,
        latest_release: manifest.latest.as_ref().and_then(|l| l.release.clone()),
        latest_snapshot: manifest.latest.as_ref().and_then(|l| l.snapshot.clone()),
    })
}

/// Fabric loader version entry from the Fabric Meta API
#[derive(Clone, Deserialize)]
struct FabricLoaderEntry {
    version: String,
}

#[tauri::command]
pub fn fetch_fabric_versions_cmd() -> Result<Vec<String>, String> {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get("https://meta.fabricmc.net/v2/versions/loader")
        .send()
        .map_err(|e| format!("Failed to fetch Fabric versions: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP error: {}", resp.status()));
    }

    let entries: Vec<FabricLoaderEntry> = resp
        .json()
        .map_err(|e| format!("Failed to parse Fabric versions: {}", e))?;

    let versions: Vec<String> = entries.into_iter().map(|e| e.version).collect();
    Ok(versions)
}

/// Quilt loader version entry from the Quilt Meta API
#[derive(Clone, Deserialize)]
struct QuiltLoaderEntry {
    version: String,
}

#[tauri::command]
pub fn fetch_quilt_versions_cmd() -> Result<Vec<String>, String> {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get("https://meta.quiltmc.org/v3/versions/loader")
        .send()
        .map_err(|e| format!("Failed to fetch Quilt versions: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP error: {}", resp.status()));
    }

    let entries: Vec<QuiltLoaderEntry> = resp
        .json()
        .map_err(|e| format!("Failed to parse Quilt versions: {}", e))?;

    let versions: Vec<String> = entries.into_iter().map(|e| e.version).collect();
    Ok(versions)
}

/// NeoForge version entry from the NeoForge API
#[derive(Clone, Deserialize)]
struct NeoForgeVersionsResponse {
    versions: Vec<String>,
}

/// Extract the minor.patch portion from a Minecraft version string.
/// NeoForge versions are based on the MC version without the leading "1." prefix.
/// For example: "1.20.1" -> "20.1", "1.21" -> "21", "2.0" -> "2.0" (future-proof)
fn extract_neoforge_version_filter(mc_version: &str) -> String {
    // Split by '.' and skip the first component (usually "1")
    let parts: Vec<&str> = mc_version.split('.').collect();
    if parts.len() >= 2 {
        // For versions like "1.20.1" -> "20.1", "1.21" -> "21"
        // For potential future "2.0" -> "0" (just the second part onwards)
        parts[1..].join(".")
    } else {
        // Fallback: return as-is if format is unexpected
        mc_version.to_string()
    }
}

#[tauri::command]
pub fn fetch_neoforge_versions_cmd(mc_version: Option<String>) -> Result<Vec<String>, String> {
    let client = reqwest::blocking::Client::new();

    // NeoForge API returns versions for a specific MC version
    // NeoForge versions omit the leading "1." from MC versions (e.g., 1.20.1 -> 20.1)
    let url = if let Some(ref mc) = mc_version {
        let filter = extract_neoforge_version_filter(mc);
        format!("https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge?filter={}.", filter)
    } else {
        "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge".to_string()
    };

    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("Failed to fetch NeoForge versions: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP error: {}", resp.status()));
    }

    let data: NeoForgeVersionsResponse = resp
        .json()
        .map_err(|e| format!("Failed to parse NeoForge versions: {}", e))?;

    // Sort versions in descending order (newest first) using semantic versioning
    let mut versions = data.versions;
    versions.sort_by(|a, b| compare_versions_desc(b, a));
    Ok(versions)
}

/// Forge promotions response
#[derive(Clone, Deserialize)]
struct ForgePromotionsResponse {
    promos: std::collections::HashMap<String, String>,
}

#[tauri::command]
pub fn fetch_forge_versions_cmd(mc_version: Option<String>) -> Result<Vec<String>, String> {
    let client = reqwest::blocking::Client::new();

    // Forge uses a promotions endpoint that lists recommended/latest versions
    let resp = client
        .get("https://files.minecraftforge.net/maven/net/minecraftforge/forge/promotions_slim.json")
        .send()
        .map_err(|e| format!("Failed to fetch Forge promotions: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP error: {}", resp.status()));
    }

    let promos: ForgePromotionsResponse = resp
        .json()
        .map_err(|e| format!("Failed to parse Forge promotions: {}", e))?;

    // Filter versions based on MC version if provided
    let mut versions: Vec<String> = if let Some(mc) = mc_version {
        // Look for versions matching this MC version exactly
        // Key format: "1.20.1-recommended" or "1.20.1-latest"
        let prefix = format!("{}-", mc);
        promos
            .promos
            .iter()
            .filter(|(key, _)| key.starts_with(&prefix))
            .map(|(_, version)| {
                // Value is the forge version number
                format!("{}-{}", mc, version)
            })
            .collect()
    } else {
        // Return all unique MC-version combinations
        let mut seen = std::collections::HashSet::new();
        promos
            .promos
            .iter()
            .filter_map(|(key, version)| {
                // Extract MC version from key (e.g., "1.20.1" from "1.20.1-recommended")
                let mc = key.split('-').next()?;
                let full_version = format!("{}-{}", mc, version);
                if seen.insert(full_version.clone()) {
                    Some(full_version)
                } else {
                    None
                }
            })
            .collect()
    };

    // Sort versions in descending order (newest first) using semantic versioning
    versions.sort_by(|a, b| compare_versions_desc(b, a));
    Ok(versions)
}

/// Compare two version strings semantically (for descending sort)
/// Returns Ordering based on semantic version comparison
fn compare_versions_desc(a: &str, b: &str) -> std::cmp::Ordering {
    let parse_parts = |s: &str| -> Vec<u64> {
        s.split(['.', '-'])
            .filter_map(|p| p.parse::<u64>().ok())
            .collect()
    };

    let a_parts = parse_parts(a);
    let b_parts = parse_parts(b);

    for (a_part, b_part) in a_parts.iter().zip(b_parts.iter()) {
        match a_part.cmp(b_part) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }

    // If all compared parts are equal, longer version is greater
    a_parts.len().cmp(&b_parts.len())
}

/// Fetch loader versions for any supported loader type
#[tauri::command]
pub fn fetch_loader_versions_cmd(
    loader_type: String,
    mc_version: Option<String>,
) -> Result<Vec<String>, String> {
    match loader_type.to_lowercase().as_str() {
        "fabric" => fetch_fabric_versions_cmd(),
        "quilt" => fetch_quilt_versions_cmd(),
        "neoforge" => fetch_neoforge_versions_cmd(mc_version),
        "forge" => fetch_forge_versions_cmd(mc_version),
        other => Err(format!("Unsupported loader type: {}", other)),
    }
}

// ============================================================================
// Java detection and validation commands
// ============================================================================

/// Detect all Java installations on the system.
#[tauri::command]
pub fn detect_java_installations_cmd() -> Vec<JavaInstallation> {
    detect_installations()
}

/// Validate a specific Java path.
#[tauri::command]
pub fn validate_java_path_cmd(path: String) -> JavaValidation {
    validate_java_path(&path)
}

/// Get the minimum required Java version for a Minecraft version.
#[tauri::command]
pub fn get_required_java_version_cmd(mc_version: String) -> u32 {
    get_required_java_version(&mc_version)
}

/// Check if a Java version is compatible with a Minecraft version.
#[tauri::command]
pub fn check_java_compatibility_cmd(java_major: u32, mc_version: String) -> bool {
    is_java_compatible(java_major, &mc_version)
}

/// Fetch Adoptium release info for a Java version.
#[tauri::command]
pub fn fetch_adoptium_release_cmd(java_major: u32) -> Result<AdoptiumRelease, String> {
    fetch_adoptium_release(java_major).map_err(|e| e.to_string())
}

/// Download and install Java from Adoptium.
#[tauri::command]
pub fn download_java_cmd(
    app: AppHandle,
    download_manager: State<'_, DownloadManager>,
    java_major: u32,
) -> Result<String, String> {
    let paths = Paths::new().map_err(|e| e.to_string())?;
    paths.ensure().map_err(|e| e.to_string())?;

    let install_dir = paths.java_runtimes.join(format!("temurin-{}", java_major));

    // Create a progress callback that emits events
    let app_handle = app.clone();
    let progress_callback = Some(Box::new(move |downloaded: u64, total: u64| {
        let _ = app_handle.emit("java-download-progress", serde_json::json!({
            "downloaded": downloaded,
            "total": total,
            "percentage": if total > 0 { (downloaded as f64 / total as f64 * 100.0) as u32 } else { 0 }
        }));
    }) as Box<dyn Fn(u64, u64) + Send + Sync>);

    let java_path = download_and_install_java_managed(
        java_major,
        &install_dir,
        download_manager.inner(),
        progress_callback,
    )
    .map_err(|e| e.to_string())?;

    Ok(java_path.to_string_lossy().to_string())
}

/// Find a compatible Java for a Minecraft version (checks managed runtimes first).
#[tauri::command]
pub fn find_compatible_java_cmd(mc_version: String) -> Result<Option<String>, String> {
    let paths = Paths::new().map_err(|e| e.to_string())?;
    Ok(find_compatible_java(&mc_version, &paths.java_runtimes))
}

/// Check if a managed Java runtime exists for a version.
#[tauri::command]
pub fn get_managed_java_cmd(java_major: u32) -> Result<Option<String>, String> {
    let paths = Paths::new().map_err(|e| e.to_string())?;
    Ok(get_managed_java(&paths.java_runtimes, java_major).map(|p| p.to_string_lossy().to_string()))
}

/// List all managed Java runtimes.
#[tauri::command]
pub fn list_managed_runtimes_cmd() -> Result<Vec<JavaInstallation>, String> {
    let paths = Paths::new().map_err(|e| e.to_string())?;
    Ok(list_managed_runtimes(&paths.java_runtimes))
}

// ============================================================================
// Library commands
// ============================================================================

#[derive(Deserialize)]
pub struct LibraryFilterInput {
    pub content_type: Option<String>,
    pub search: Option<String>,
    pub tags: Option<Vec<String>>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Deserialize)]
pub struct LibraryItemUpdateInput {
    pub name: Option<String>,
    pub notes: Option<String>,
}

#[tauri::command]
pub fn library_list_items_cmd(filter: LibraryFilterInput) -> Result<Vec<LibraryItem>, String> {
    let paths = load_paths()?;
    let library = Library::from_paths(&paths).map_err(|e| e.to_string())?;
    let filter = LibraryFilter {
        content_type: filter.content_type,
        search: filter.search,
        tags: filter.tags,
        limit: filter.limit,
        offset: filter.offset,
    };
    library.list_items(&filter).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn library_get_item_cmd(id: i64) -> Result<Option<LibraryItem>, String> {
    let paths = load_paths()?;
    let library = Library::from_paths(&paths).map_err(|e| e.to_string())?;
    library.get_item(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn library_get_item_by_hash_cmd(hash: String) -> Result<Option<LibraryItem>, String> {
    let paths = load_paths()?;
    let library = Library::from_paths(&paths).map_err(|e| e.to_string())?;
    library.get_item_by_hash(&hash).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn library_add_item_cmd(input: LibraryItemInput) -> Result<LibraryItem, String> {
    let paths = load_paths()?;
    let library = Library::from_paths(&paths).map_err(|e| e.to_string())?;
    library.add_item(&input).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn library_update_item_cmd(
    id: i64,
    input: LibraryItemUpdateInput,
) -> Result<LibraryItem, String> {
    let paths = load_paths()?;
    let library = Library::from_paths(&paths).map_err(|e| e.to_string())?;
    let item = library
        .get_item(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "item not found".to_string())?;
    let update = LibraryItemInput {
        hash: item.hash,
        name: input.name,
        notes: input.notes,
        ..Default::default()
    };
    library.update_item(id, &update).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn library_delete_item_cmd(id: i64, delete_file: bool) -> Result<bool, String> {
    let paths = load_paths()?;
    let library = Library::from_paths(&paths).map_err(|e| e.to_string())?;

    if delete_file {
        if let Some(item) = library.get_item(id).map_err(|e| e.to_string())? {
            let store_path = match item.content_type {
                LibraryContentType::Mod => paths.store_mod_path(&item.hash),
                LibraryContentType::ModPack => paths.store_modpack_path(&item.hash),
                LibraryContentType::ResourcePack => paths.store_resourcepack_path(&item.hash),
                LibraryContentType::ShaderPack => paths.store_shaderpack_path(&item.hash),
                LibraryContentType::Skin => paths.store_skin_path(&item.hash),
            };
            if store_path.exists() {
                std::fs::remove_file(&store_path).map_err(|e| e.to_string())?;
            }
        }
    }

    library.delete_item(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn library_get_item_path_cmd(id: i64) -> Result<Option<String>, String> {
    let paths = load_paths()?;
    let library = Library::from_paths(&paths).map_err(|e| e.to_string())?;

    if let Some(item) = library.get_item(id).map_err(|e| e.to_string())? {
        let store_path = match item.content_type {
            LibraryContentType::Mod => paths.store_mod_path(&item.hash),
            LibraryContentType::ModPack => paths.store_modpack_path(&item.hash),
            LibraryContentType::ResourcePack => paths.store_resourcepack_path(&item.hash),
            LibraryContentType::ShaderPack => paths.store_shaderpack_path(&item.hash),
            LibraryContentType::Skin => paths.store_skin_path(&item.hash),
        };
        if store_path.exists() {
            Ok(Some(store_path.to_string_lossy().to_string()))
        } else {
            Ok(None)
        }
    } else {
        Ok(None)
    }
}

#[tauri::command]
pub fn library_import_file_cmd(path: String, content_type: String) -> Result<LibraryItem, String> {
    let paths = load_paths()?;
    let library = Library::from_paths(&paths).map_err(|e| e.to_string())?;
    let ct = LibraryContentType::from_str(&content_type)
        .ok_or_else(|| "invalid content type".to_string())?;
    library
        .import_file(&paths, &PathBuf::from(path), ct)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn library_import_folder_cmd(
    path: String,
    content_type: String,
    recursive: bool,
) -> Result<ImportResult, String> {
    let paths = load_paths()?;
    let library = Library::from_paths(&paths).map_err(|e| e.to_string())?;
    let ct = LibraryContentType::from_str(&content_type)
        .ok_or_else(|| "invalid content type".to_string())?;
    library
        .import_folder(&paths, &PathBuf::from(path), ct, recursive)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn library_get_stats_cmd() -> Result<LibraryStats, String> {
    let paths = load_paths()?;
    let library = Library::from_paths(&paths).map_err(|e| e.to_string())?;
    library.stats().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn library_sync_cmd() -> Result<ImportResult, String> {
    let paths = load_paths()?;
    let library = Library::from_paths(&paths).map_err(|e| e.to_string())?;
    let mut result = library.sync_with_store(&paths).map_err(|e| e.to_string())?;

    // After syncing, enrich library items with metadata from profiles
    if let Err(e) = enrich_library_from_profiles(&paths, &library) {
        result
            .errors
            .push(format!("Warning: Failed to enrich library metadata: {}", e));
    }

    Ok(result)
}

/// Enrich library items with metadata from all profiles
fn enrich_library_from_profiles(paths: &Paths, library: &Library) -> Result<usize, String> {
    let profiles = list_profiles(paths).map_err(|e| e.to_string())?;
    let mut enriched = 0;

    for profile_id in profiles {
        if let Ok(profile) = load_profile(paths, &profile_id) {
            // Enrich from mods
            for content in &profile.mods {
                if library
                    .enrich_item_from_content_ref(
                        &content.hash,
                        &content.name,
                        content.file_name.as_deref(),
                        content.source.as_deref(),
                        content.platform.as_deref(),
                        content.project_id.as_deref(),
                        content.version.as_deref(),
                    )
                    .is_ok()
                {
                    enriched += 1;
                }
            }

            // Enrich from resourcepacks
            for content in &profile.resourcepacks {
                if library
                    .enrich_item_from_content_ref(
                        &content.hash,
                        &content.name,
                        content.file_name.as_deref(),
                        content.source.as_deref(),
                        content.platform.as_deref(),
                        content.project_id.as_deref(),
                        content.version.as_deref(),
                    )
                    .is_ok()
                {
                    enriched += 1;
                }
            }

            // Enrich from shaderpacks
            for content in &profile.shaderpacks {
                if library
                    .enrich_item_from_content_ref(
                        &content.hash,
                        &content.name,
                        content.file_name.as_deref(),
                        content.source.as_deref(),
                        content.platform.as_deref(),
                        content.project_id.as_deref(),
                        content.version.as_deref(),
                    )
                    .is_ok()
                {
                    enriched += 1;
                }
            }
        }
    }

    Ok(enriched)
}

#[tauri::command]
pub fn library_enrich_from_profiles_cmd() -> Result<usize, String> {
    let paths = load_paths()?;
    let library = Library::from_paths(&paths).map_err(|e| e.to_string())?;
    enrich_library_from_profiles(&paths, &library)
}

#[tauri::command]
pub fn library_list_tags_cmd() -> Result<Vec<Tag>, String> {
    let paths = load_paths()?;
    let library = Library::from_paths(&paths).map_err(|e| e.to_string())?;
    library.list_tags().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn library_create_tag_cmd(name: String, color: Option<String>) -> Result<Tag, String> {
    let paths = load_paths()?;
    let library = Library::from_paths(&paths).map_err(|e| e.to_string())?;
    library
        .create_tag(&name, color.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn library_delete_tag_cmd(id: i64) -> Result<bool, String> {
    let paths = load_paths()?;
    let library = Library::from_paths(&paths).map_err(|e| e.to_string())?;
    library.delete_tag(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn library_set_item_tags_cmd(item_id: i64, tag_names: Vec<String>) -> Result<(), String> {
    let paths = load_paths()?;
    let library = Library::from_paths(&paths).map_err(|e| e.to_string())?;
    library
        .set_item_tags(item_id, &tag_names)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn library_add_to_profile_cmd(profile_id: String, item_id: i64) -> Result<Profile, String> {
    let paths = load_paths()?;
    snapshot_before_change(&paths, &profile_id, "before adding library content")?;
    let library = Library::from_paths(&paths).map_err(|e| e.to_string())?;
    let mut profile = load_profile(&paths, &profile_id).map_err(|e| e.to_string())?;

    let item = library
        .get_item(item_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "item not found".to_string())?;

    let content_ref = ContentRef {
        name: item.name.clone(),
        hash: format!("sha256:{}", item.hash),
        version: item.source_version.clone(),
        source: item.source_url.clone(),
        file_name: item.file_name.clone(),
        platform: item.source_platform.clone(),
        project_id: item.source_project_id.clone(),
        version_id: None, // Library items may not have version IDs
        enabled: true,
        pinned: false,
    };

    match item.content_type {
        LibraryContentType::Mod => {
            upsert_mod(&mut profile, content_ref);
        }
        LibraryContentType::ModPack => {
            return Err("modpacks are installed as new profiles from Store".to_string())
        }
        LibraryContentType::ResourcePack => {
            upsert_resourcepack(&mut profile, content_ref);
        }
        LibraryContentType::ShaderPack => {
            upsert_shaderpack(&mut profile, content_ref);
        }
        LibraryContentType::Skin => return Err("skins cannot be added to profiles".to_string()),
    };

    // Link in library
    library
        .link_item_to_profile(item_id, &profile_id, item.content_type)
        .map_err(|e| e.to_string())?;

    save_profile(&paths, &profile).map_err(|e| e.to_string())?;
    Ok(profile)
}

// ============================================================================
// Settings and Storage Stats Commands
// ============================================================================

#[tauri::command]
pub fn get_data_path_cmd() -> Result<String, String> {
    let paths = load_paths()?;
    // Derive the base path from the profiles directory (profiles is at base/profiles)
    let base = paths
        .profiles
        .parent()
        .ok_or_else(|| "could not determine data path".to_string())?;
    Ok(base.to_string_lossy().to_string())
}

#[tauri::command]
pub fn get_storage_stats_cmd() -> Result<StorageStats, String> {
    let paths = load_paths()?;
    get_storage_stats(&paths).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_unused_items_cmd() -> Result<UnusedItemsSummary, String> {
    let paths = load_paths()?;
    let library = Library::from_paths(&paths).map_err(|e| e.to_string())?;
    library.get_unused_items().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn purge_unused_items_cmd(content_types: Vec<String>) -> Result<PurgeResult, String> {
    let paths = load_paths()?;
    let library = Library::from_paths(&paths).map_err(|e| e.to_string())?;

    // Convert string content types to LibraryContentType
    let types: Vec<LibraryContentType> = content_types
        .iter()
        .filter_map(|s| LibraryContentType::from_str(s))
        .collect();

    // Always delete files from store when purging
    library
        .purge_unused_items(&paths, &types, true)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_auto_update_enabled_cmd() -> Result<bool, String> {
    let paths = load_paths()?;
    let config = load_config(&paths).map_err(|e| e.to_string())?;
    Ok(config.auto_update_enabled)
}

#[tauri::command]
pub fn set_auto_update_enabled_cmd(enabled: bool) -> Result<Config, String> {
    let paths = load_paths()?;
    let mut config = load_config(&paths).map_err(|e| e.to_string())?;
    config.auto_update_enabled = enabled;
    save_config(&paths, &config).map_err(|e| e.to_string())?;
    Ok(redact_config_secrets(config))
}

// ============================================================================
// Update Checking Commands
// ============================================================================

#[tauri::command]
pub fn check_all_updates_cmd() -> Result<UpdateCheckResult, String> {
    let paths = load_paths()?;
    let config = load_config(&paths).map_err(|e| e.to_string())?;
    check_all_updates(&paths, config.curseforge_api_key.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn check_profile_updates_cmd(profile_id: String) -> Result<UpdateCheckResult, String> {
    let paths = load_paths()?;
    let config = load_config(&paths).map_err(|e| e.to_string())?;
    check_profile_updates(&paths, &profile_id, config.curseforge_api_key.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn apply_content_update_cmd(
    profile_id: String,
    content_name: String,
    content_type: String,
    new_version_id: String,
) -> Result<Profile, String> {
    let paths = load_paths()?;
    snapshot_before_change(&paths, &profile_id, "before content update")?;
    let config = load_config(&paths).map_err(|e| e.to_string())?;
    apply_update(
        &paths,
        &profile_id,
        &content_name,
        &content_type,
        &new_version_id,
        config.curseforge_api_key.as_deref(),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_content_pinned_cmd(
    profile_id: String,
    content_name: String,
    content_type: String,
    pinned: bool,
) -> Result<Profile, String> {
    let paths = load_paths()?;
    snapshot_before_change(&paths, &profile_id, "before pin change")?;
    set_content_pinned(&paths, &profile_id, &content_name, &content_type, pinned)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_content_enabled_cmd(
    profile_id: String,
    content_name: String,
    content_type: String,
    enabled: bool,
) -> Result<Profile, String> {
    let paths = load_paths()?;
    snapshot_before_change(&paths, &profile_id, "before content state change")?;
    set_content_enabled(&paths, &profile_id, &content_name, &content_type, enabled)
        .map_err(|e| e.to_string())
}

// Profile organization types (mirrors frontend types)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileFolder {
    pub id: String,
    pub name: String,
    pub profiles: Vec<String>,
    pub collapsed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProfileOrganization {
    pub folders: Vec<ProfileFolder>,
    pub ungrouped: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favorite_profile: Option<String>,
}

#[tauri::command]
pub fn load_profile_organization_cmd() -> Result<ProfileOrganization, String> {
    let paths = load_paths()?;
    if paths.profile_organization.exists() {
        let data = std::fs::read_to_string(&paths.profile_organization)
            .map_err(|e| format!("Failed to read profile organization: {}", e))?;
        serde_json::from_str(&data)
            .map_err(|e| format!("Failed to parse profile organization: {}", e))
    } else {
        Ok(ProfileOrganization::default())
    }
}

#[tauri::command]
pub fn save_profile_organization_cmd(organization: ProfileOrganization) -> Result<(), String> {
    let paths = load_paths()?;
    let data = serde_json::to_string_pretty(&organization)
        .map_err(|e| format!("Failed to serialize profile organization: {}", e))?;
    velgrinor::util::atomic_write(&paths.profile_organization, data)
        .map_err(|e| format!("Failed to write profile organization: {}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_responses_do_not_expose_secrets() {
        let config = Config {
            msa_client_id: Some("public-client-id".to_string()),
            msa_client_secret: Some("microsoft-secret".to_string()),
            curseforge_api_key: Some("curseforge-secret".to_string()),
            ..Config::default()
        };

        let redacted = redact_config_secrets(config);

        assert_eq!(redacted.msa_client_id.as_deref(), Some("public-client-id"));
        assert!(redacted.msa_client_secret.is_none());
        assert!(redacted.curseforge_api_key.is_none());
    }
}
