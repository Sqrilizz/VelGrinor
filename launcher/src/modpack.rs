use crate::download::{DownloadManager, DownloadRequest, DownloadSnapshot};
use crate::instance::materialize_instance;
use crate::library::{Library, LibraryContentType, LibraryItemInput};
use crate::paths::Paths;
use crate::profile::{
    ContentRef, Files, Loader, Profile, Runtime, load_profile, upsert_mod, upsert_resourcepack,
    upsert_shaderpack,
};
use crate::store::{
    ContentKind, content_store_path, hash_file, normalize_hash, store_content, store_from_url,
};
use crate::util::{atomic_write, sanitize_filename};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha1::{Digest as Sha1Digest, Sha1};
use sha2::Sha512;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

#[derive(Debug, Clone, Serialize)]
pub struct ProfileRepairReport {
    pub checked: usize,
    pub repaired: usize,
    pub missing: Vec<String>,
    pub corrupt: Vec<String>,
    pub errors: Vec<String>,
    pub conflicts: Vec<String>,
}

#[derive(Serialize)]
struct ExportIndex {
    #[serde(rename = "formatVersion")]
    format_version: u32,
    game: String,
    #[serde(rename = "versionId")]
    version_id: String,
    name: String,
    summary: String,
    files: Vec<ExportFile>,
    dependencies: HashMap<String, String>,
}

#[derive(Serialize)]
struct ExportFile {
    path: String,
    hashes: ExportHashes,
    downloads: Vec<String>,
    #[serde(rename = "fileSize")]
    file_size: u64,
}

#[derive(Serialize)]
struct ExportHashes {
    sha1: String,
    sha512: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ModrinthIndex {
    #[serde(rename = "formatVersion")]
    format_version: u32,
    game: String,
    #[serde(rename = "versionId")]
    version_id: String,
    name: String,
    #[serde(default)]
    summary: Option<String>,
    files: Vec<ModrinthFile>,
    dependencies: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct ModrinthFile {
    path: String,
    hashes: ModrinthHashes,
    downloads: Vec<String>,
    #[serde(default)]
    env: Option<ModrinthEnv>,
    #[serde(rename = "fileSize")]
    file_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ModrinthHashes {
    sha1: String,
    sha512: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ModrinthEnv {
    client: Option<String>,
    server: Option<String>,
}

pub fn import_mrpack(paths: &Paths, pack_path: &Path, profile_id: Option<&str>) -> Result<Profile> {
    import_mrpack_with_progress(paths, pack_path, profile_id, &mut |_, _| {})
}

pub fn import_mrpack_with_progress(
    paths: &Paths,
    pack_path: &Path,
    profile_id: Option<&str>,
    progress: &mut dyn FnMut(u8, String),
) -> Result<Profile> {
    let manager = DownloadManager::new(paths.downloads_state(), 3)?;
    import_mrpack_with_download_manager(paths, pack_path, profile_id, &manager, progress, |_| {})
}

pub fn import_mrpack_with_download_manager(
    paths: &Paths,
    pack_path: &Path,
    profile_id: Option<&str>,
    download_manager: &DownloadManager,
    progress: &mut dyn FnMut(u8, String),
    download_progress: impl Fn(DownloadSnapshot) + Send + Sync,
) -> Result<Profile> {
    progress(30, "Reading modpack manifest".to_string());
    let file = fs::File::open(pack_path)
        .with_context(|| format!("failed to open modpack: {}", pack_path.display()))?;
    let mut zip = ZipArchive::new(file).context("failed to read modpack zip")?;

    let index = read_modrinth_index(&mut zip)?;
    validate_index(&index)?;

    let (mc_version, loader) = resolve_dependencies(&index.dependencies)?;

    let profile_id = resolve_profile_id(paths, &index.name, profile_id)?;
    if paths.is_profile_present(&profile_id) {
        bail!("profile already exists: {}", profile_id);
    }

    let staging = paths.profiles.join(format!(".{profile_id}.staging"));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(staging.join("overrides"))?;
    let mut staging_guard = StagingGuard::new(staging.clone());
    let overrides_dir = staging.join("overrides");
    progress(37, "Extracting overrides".to_string());
    extract_overrides(&mut zip, &overrides_dir)?;

    let mut profile = Profile {
        id: profile_id.clone(),
        mc_version,
        loader,
        mods: Vec::new(),
        resourcepacks: Vec::new(),
        shaderpacks: Vec::new(),
        runtime: Runtime::default(),
        files: Files::default(),
    };
    let client_files = index
        .files
        .iter()
        .filter(|file| is_client_allowed(&file.env))
        .collect::<Vec<_>>();
    let total_files = client_files.len();
    let mut pending = Vec::new();
    for (index, file) in client_files.iter().enumerate() {
        let url = file
            .downloads
            .first()
            .context("modpack file has no downloads")?;
        let destination = paths.cache_downloads.join(format!(
            "mrpack-{profile_id}-{index}-{}",
            sanitize_filename(
                Path::new(&file.path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("file")
            )
        ));
        let mut request = DownloadRequest::new(url, &destination);
        request.mirrors = file.downloads.iter().skip(1).cloned().collect();
        request.expected_size = file.file_size;
        request.sha1 = Some(file.hashes.sha1.clone());
        request.group = Some(format!("mrpack-{profile_id}"));
        request.label = Some(file.path.clone());
        pending.push((file, destination, url.clone(), request));
    }
    download_manager.run_requests(
        pending.iter().map(|item| item.3.clone()).collect(),
        download_progress,
    )?;
    let mut library_items = Vec::new();
    for (index, (file, download_path, download_url, _)) in pending.into_iter().enumerate() {
        let current = index + 1;
        let percent = 40 + ((current * 55) / total_files.max(1)) as u8;
        progress(
            percent,
            format!("Installing files {current}/{total_files}: {}", file.path),
        );
        let rel_path = sanitize_rel_path(&file.path)?;

        match content_kind_for_path(&file.path) {
            Some(kind) => {
                let file_name_override = rel_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string());
                let stored = store_content(
                    paths,
                    kind,
                    &download_path,
                    Some(download_url.clone()),
                    file_name_override,
                )?;
                let source = modrinth_source(&download_url);
                library_items.push((
                    kind,
                    LibraryItemInput {
                        hash: stored.hash.clone(),
                        content_type: Some(kind.label().to_string()),
                        name: Some(stored.name.clone()),
                        file_name: Some(stored.file_name.clone()),
                        file_size: fs::metadata(&download_path)
                            .ok()
                            .map(|meta| meta.len() as i64),
                        source_url: Some(download_url.clone()),
                        source_platform: source.as_ref().map(|_| "modrinth".to_string()),
                        source_project_id: source.as_ref().map(|(project, _)| project.clone()),
                        source_version: source.as_ref().map(|(_, version)| version.clone()),
                        notes: None,
                    },
                ));
                let content_ref = ContentRef {
                    name: stored.name,
                    hash: stored.hash,
                    version: None,
                    source: stored.source,
                    file_name: Some(stored.file_name),
                    platform: source.as_ref().map(|_| "modrinth".to_string()),
                    project_id: source.as_ref().map(|(project, _)| project.clone()),
                    version_id: source.as_ref().map(|(_, version)| version.clone()),
                    enabled: true,
                    pinned: false,
                };
                match kind {
                    ContentKind::Mod => {
                        upsert_mod(&mut profile, content_ref);
                    }
                    ContentKind::ModPack => {}
                    ContentKind::ResourcePack => {
                        upsert_resourcepack(&mut profile, content_ref);
                    }
                    ContentKind::ShaderPack => {
                        upsert_shaderpack(&mut profile, content_ref);
                    }
                    ContentKind::Skin => {}
                }
            }
            None => {
                write_override_file(&overrides_dir, &rel_path, &download_path)?;
            }
        }
        let _ = fs::remove_file(download_path);
    }

    progress(98, "Saving profile".to_string());
    atomic_write(
        &staging.join("profile.json"),
        serde_json::to_vec_pretty(&profile)?,
    )?;
    fs::rename(&staging, paths.profile_dir(&profile_id))?;
    staging_guard.keep();
    if let Ok(library) = Library::from_paths(paths) {
        for (kind, input) in library_items {
            if let Ok(item) = library.add_item(&input) {
                let _ = library.link_item_to_profile(
                    item.id,
                    &profile_id,
                    LibraryContentType::from_content_kind(kind),
                );
            }
        }
    }
    progress(100, "Modpack installed".to_string());
    Ok(profile)
}

struct StagingGuard {
    path: PathBuf,
    keep: bool,
}

impl StagingGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, keep: false }
    }

    fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn modrinth_source(url: &str) -> Option<(String, String)> {
    let marker = "cdn.modrinth.com/data/";
    let tail = url.split_once(marker)?.1;
    let mut parts = tail.split('/');
    let project = parts.next()?.to_string();
    if parts.next()? != "versions" {
        return None;
    }
    let version = parts.next()?.to_string();
    Some((project, version))
}

fn read_modrinth_index<R: Read + Seekable>(zip: &mut ZipArchive<R>) -> Result<ModrinthIndex> {
    let mut index_file = zip
        .by_name("modrinth.index.json")
        .context("modrinth.index.json not found in modpack")?;
    let mut data = String::new();
    index_file
        .read_to_string(&mut data)
        .context("failed to read modrinth.index.json")?;
    let index: ModrinthIndex =
        serde_json::from_str(&data).context("failed to parse modrinth.index.json")?;
    Ok(index)
}

fn validate_index(index: &ModrinthIndex) -> Result<()> {
    if index.format_version != 1 {
        bail!(
            "unsupported modpack format version: {}",
            index.format_version
        );
    }
    if index.game != "minecraft" {
        bail!("unsupported modpack game: {}", index.game);
    }
    Ok(())
}

fn resolve_dependencies(deps: &HashMap<String, String>) -> Result<(String, Option<Loader>)> {
    let mc_version = deps
        .get("minecraft")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("modpack missing minecraft dependency"))?;

    let loader = if let Some(version) = deps.get("fabric-loader") {
        Some(Loader {
            loader_type: "fabric".to_string(),
            version: version.clone(),
        })
    } else if let Some(version) = deps.get("quilt-loader") {
        Some(Loader {
            loader_type: "quilt".to_string(),
            version: version.clone(),
        })
    } else if let Some(version) = deps.get("neoforge") {
        Some(Loader {
            loader_type: "neoforge".to_string(),
            version: version.clone(),
        })
    } else {
        deps.get("forge").map(|version| Loader {
            loader_type: "forge".to_string(),
            version: version.clone(),
        })
    };

    Ok((mc_version, loader))
}

fn resolve_profile_id(paths: &Paths, name: &str, requested: Option<&str>) -> Result<String> {
    if let Some(id) = requested {
        let trimmed = id.trim();
        if trimmed.is_empty() {
            bail!("profile id cannot be empty");
        }
        return Ok(trimmed.to_string());
    }

    let base = slugify(name);
    let base = if base.is_empty() {
        "modpack".to_string()
    } else {
        base
    };
    let mut candidate = base.clone();
    let mut idx = 1;
    while paths.is_profile_present(&candidate) {
        idx += 1;
        candidate = format!("{}-{}", base, idx);
    }
    Ok(candidate)
}

fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in input.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn content_kind_for_path(path: &str) -> Option<ContentKind> {
    let normalized = path.replace('\\', "/");
    let normalized = normalized.trim_start_matches("./");
    if normalized.starts_with("mods/") {
        Some(ContentKind::Mod)
    } else if normalized.starts_with("resourcepacks/") {
        Some(ContentKind::ResourcePack)
    } else if normalized.starts_with("shaderpacks/") {
        Some(ContentKind::ShaderPack)
    } else {
        None
    }
}

fn is_client_allowed(env: &Option<ModrinthEnv>) -> bool {
    !matches!(
        env.as_ref().and_then(|e| e.client.as_ref()),
        Some(flag) if flag == "unsupported"
    )
}

fn sanitize_rel_path(path: &str) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for comp in Path::new(path).components() {
        match comp {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => bail!("invalid path in modpack: {}", path),
        }
    }
    if out.as_os_str().is_empty() {
        bail!("invalid empty path in modpack");
    }
    Ok(out)
}

fn sha1_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("failed to open file for hashing: {}", path.display()))?;
    let mut hasher = Sha1::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf).context("failed to read file")?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn extract_overrides<R: Read + Seekable>(
    zip: &mut ZipArchive<R>,
    overrides_dir: &Path,
) -> Result<()> {
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).context("failed to read zip entry")?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().to_string();
        let rest = if let Some(rest) = name.strip_prefix("overrides/") {
            rest
        } else if let Some(rest) = name.strip_prefix("client-overrides/") {
            rest
        } else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        let rel = sanitize_rel_path(rest)?;
        let target = overrides_dir.join(rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = fs::File::create(&target)
            .with_context(|| format!("failed to write override file: {}", target.display()))?;
        std::io::copy(&mut file, &mut out)
            .with_context(|| format!("failed to extract override file: {}", name))?;
        out.flush().ok();
    }
    Ok(())
}

fn write_override_file(overrides_dir: &Path, rel_path: &Path, src: &Path) -> Result<()> {
    let target = overrides_dir.join(rel_path);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(src, &target)
        .with_context(|| format!("failed to copy override file to {}", target.display()))?;
    Ok(())
}

pub fn repair_profile(paths: &Paths, profile_id: &str) -> Result<ProfileRepairReport> {
    let profile = load_profile(paths, profile_id)?;
    let mut report = ProfileRepairReport {
        checked: 0,
        repaired: 0,
        missing: Vec::new(),
        corrupt: Vec::new(),
        errors: Vec::new(),
        conflicts: profile_conflicts(&profile),
    };
    for (kind, content) in [
        (ContentKind::Mod, profile.mods.as_slice()),
        (ContentKind::ResourcePack, profile.resourcepacks.as_slice()),
        (ContentKind::ShaderPack, profile.shaderpacks.as_slice()),
    ] {
        for item in content {
            report.checked += 1;
            let expected = normalize_hash(&item.hash);
            let target = content_store_path(paths, kind, expected);
            let state = if !target.exists() {
                report.missing.push(item.name.clone());
                Some("missing")
            } else if hash_file(&target)
                .map(|hash| hash != expected)
                .unwrap_or(true)
            {
                report.corrupt.push(item.name.clone());
                Some("corrupt")
            } else {
                None
            };
            if state.is_none() {
                continue;
            }
            let Some(source) = item
                .source
                .as_deref()
                .filter(|value| value.starts_with("http"))
            else {
                report
                    .errors
                    .push(format!("{}: no download source", item.name));
                continue;
            };
            match store_from_url(paths, source).and_then(|(download, _)| {
                let actual = hash_file(&download)?;
                if actual != expected {
                    bail!("downloaded hash does not match profile");
                }
                fs::copy(download, &target)?;
                Ok(())
            }) {
                Ok(()) => report.repaired += 1,
                Err(error) => report.errors.push(format!("{}: {error}", item.name)),
            }
        }
    }
    materialize_instance(paths, &profile)?;
    Ok(report)
}

pub fn backup_profile(paths: &Paths, profile_id: &str) -> Result<PathBuf> {
    let source = paths.profile_dir(profile_id);
    if !source.exists() {
        bail!("profile not found: {profile_id}");
    }
    let backup_dir = paths
        .profiles
        .parent()
        .context("profile storage has no parent")?
        .join("backups");
    fs::create_dir_all(&backup_dir)?;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let output = backup_dir.join(format!("{profile_id}-{timestamp}.zip"));
    let file = fs::File::create(&output)?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    add_directory_named_to_zip(&mut zip, &source, &source, "profile", options)?;
    zip.finish()?;
    Ok(output)
}

fn profile_conflicts(profile: &Profile) -> Vec<String> {
    let mut conflicts = Vec::new();
    if profile.loader.is_none() && !profile.mods.is_empty() {
        conflicts.push("profile contains mods but has no mod loader".to_string());
    }
    for (label, content) in [
        ("mod", profile.mods.as_slice()),
        ("resource pack", profile.resourcepacks.as_slice()),
        ("shader pack", profile.shaderpacks.as_slice()),
    ] {
        let mut files = HashMap::<String, usize>::new();
        let mut projects = HashMap::<String, usize>::new();
        for item in content.iter().filter(|item| item.enabled) {
            if let Some(file_name) = &item.file_name {
                *files.entry(file_name.to_lowercase()).or_default() += 1;
            }
            if let Some(project_id) = &item.project_id {
                *projects.entry(project_id.clone()).or_default() += 1;
            }
        }
        for (name, count) in files.into_iter().filter(|(_, count)| *count > 1) {
            conflicts.push(format!("duplicate {label} file: {name} ({count})"));
        }
        for (project, count) in projects.into_iter().filter(|(_, count)| *count > 1) {
            conflicts.push(format!("duplicate {label} project: {project} ({count})"));
        }
    }
    conflicts
}

pub fn export_mrpack(paths: &Paths, profile_id: &str, output: &Path) -> Result<()> {
    let profile = load_profile(paths, profile_id)?;
    let file = fs::File::create(output)
        .with_context(|| format!("failed to create export: {}", output.display()))?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut files = Vec::new();

    for (kind, folder, content) in [
        (ContentKind::Mod, "mods", profile.mods.as_slice()),
        (
            ContentKind::ResourcePack,
            "resourcepacks",
            profile.resourcepacks.as_slice(),
        ),
        (
            ContentKind::ShaderPack,
            "shaderpacks",
            profile.shaderpacks.as_slice(),
        ),
    ] {
        for item in content.iter().filter(|item| item.enabled) {
            let source_path = content_store_path(paths, kind, &item.hash);
            if !source_path.exists() {
                bail!("profile content is missing: {}", item.name);
            }
            let file_name = item.file_name.as_deref().unwrap_or(&item.name);
            let pack_path = format!("{folder}/{file_name}");
            if let Some(source) = item
                .source
                .as_deref()
                .filter(|value| value.starts_with("http"))
            {
                files.push(ExportFile {
                    path: pack_path,
                    hashes: ExportHashes {
                        sha1: sha1_file(&source_path)?,
                        sha512: sha512_file(&source_path)?,
                    },
                    downloads: vec![source.to_string()],
                    file_size: fs::metadata(&source_path)?.len(),
                });
            } else {
                add_file_to_zip(
                    &mut zip,
                    &source_path,
                    &format!("overrides/{pack_path}"),
                    options,
                )?;
            }
        }
    }

    let overrides = paths.profile_overrides(profile_id);
    if overrides.exists() {
        add_directory_to_zip(&mut zip, &overrides, &overrides, options)?;
    }

    let mut dependencies = HashMap::from([("minecraft".to_string(), profile.mc_version.clone())]);
    if let Some(loader) = &profile.loader {
        let key = match loader.loader_type.as_str() {
            "fabric" => "fabric-loader",
            "quilt" => "quilt-loader",
            "forge" => "forge",
            "neoforge" => "neoforge",
            other => other,
        };
        dependencies.insert(key.to_string(), loader.version.clone());
    }
    let index = ExportIndex {
        format_version: 1,
        game: "minecraft".to_string(),
        version_id: profile.id.clone(),
        name: profile.id.clone(),
        summary: "Exported from VelGrinor".to_string(),
        files,
        dependencies,
    };
    zip.start_file("modrinth.index.json", options)?;
    zip.write_all(&serde_json::to_vec_pretty(&index)?)?;
    zip.finish()?;
    Ok(())
}

fn sha512_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha512::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn add_file_to_zip(
    zip: &mut ZipWriter<fs::File>,
    source: &Path,
    name: &str,
    options: SimpleFileOptions,
) -> Result<()> {
    zip.start_file(name.replace('\\', "/"), options)?;
    let mut file = fs::File::open(source)?;
    std::io::copy(&mut file, zip)?;
    Ok(())
}

fn add_directory_to_zip(
    zip: &mut ZipWriter<fs::File>,
    root: &Path,
    current: &Path,
    options: SimpleFileOptions,
) -> Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            add_directory_to_zip(zip, root, &path, options)?;
        } else {
            let relative = path.strip_prefix(root)?;
            add_file_to_zip(
                zip,
                &path,
                &format!("overrides/{}", relative.to_string_lossy()),
                options,
            )?;
        }
    }
    Ok(())
}

fn add_directory_named_to_zip(
    zip: &mut ZipWriter<fs::File>,
    root: &Path,
    current: &Path,
    prefix: &str,
    options: SimpleFileOptions,
) -> Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            add_directory_named_to_zip(zip, root, &path, prefix, options)?;
        } else {
            let relative = path.strip_prefix(root)?;
            add_file_to_zip(
                zip,
                &path,
                &format!("{prefix}/{}", relative.to_string_lossy()),
                options,
            )?;
        }
    }
    Ok(())
}

// Trait alias workaround to keep ZipArchive generic bounds tidy
trait Seekable: std::io::Seek {}
impl<T: std::io::Seek> Seekable for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_paths() -> Paths {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("velgrinor-modpack-test-{unique}"));
        Paths {
            store_mods: base.join("store/mods/sha256"),
            store_modpacks: base.join("store/modpacks/sha256"),
            store_resourcepacks: base.join("store/resourcepacks/sha256"),
            store_shaderpacks: base.join("store/shaderpacks/sha256"),
            store_skins: base.join("store/skins/sha256"),
            profiles: base.join("profiles"),
            instances: base.join("instances"),
            cache_downloads: base.join("caches/downloads"),
            cache_manifests: base.join("caches/manifests"),
            logs: base.join("logs"),
            minecraft_versions: base.join("minecraft/versions"),
            minecraft_libraries: base.join("minecraft/libraries"),
            minecraft_assets_objects: base.join("minecraft/assets/objects"),
            minecraft_assets_indexes: base.join("minecraft/assets/indexes"),
            accounts: base.join("accounts.json"),
            tokens: base.join("tokens.json"),
            secrets: base.join("secrets.json"),
            config: base.join("config.json"),
            library_db: base.join("library.db"),
            profile_organization: base.join("profile-organization.json"),
            java_runtimes: base.join("java"),
        }
    }

    #[test]
    fn resolves_forge_modpack_dependencies() {
        let dependencies = HashMap::from([
            ("minecraft".to_string(), "1.20.1".to_string()),
            ("forge".to_string(), "47.3.0".to_string()),
        ]);
        let (minecraft, loader) = resolve_dependencies(&dependencies).unwrap();
        let loader = loader.unwrap();
        assert_eq!(minecraft, "1.20.1");
        assert_eq!(loader.loader_type, "forge");
        assert_eq!(loader.version, "47.3.0");
    }

    #[test]
    fn resolves_neoforge_modpack_dependencies() {
        let dependencies = HashMap::from([
            ("minecraft".to_string(), "1.21.1".to_string()),
            ("neoforge".to_string(), "21.1.200".to_string()),
        ]);
        let (minecraft, loader) = resolve_dependencies(&dependencies).unwrap();
        let loader = loader.unwrap();
        assert_eq!(minecraft, "1.21.1");
        assert_eq!(loader.loader_type, "neoforge");
        assert_eq!(loader.version, "21.1.200");
    }

    #[test]
    fn failed_import_does_not_publish_or_leave_staging_profile() {
        let paths = test_paths();
        paths.ensure().unwrap();
        let pack = paths.cache_downloads.join("broken.mrpack");
        let file = fs::File::create(&pack).unwrap();
        let mut archive = ZipWriter::new(file);
        archive
            .start_file("modrinth.index.json", SimpleFileOptions::default())
            .unwrap();
        archive
            .write_all(
                serde_json::json!({
                    "formatVersion": 1,
                    "game": "minecraft",
                    "versionId": "broken",
                    "name": "Broken Pack",
                    "files": [{
                        "path": "mods/broken.jar",
                        "hashes": { "sha1": "00", "sha512": "00" },
                        "downloads": ["http://127.0.0.1:1/unavailable"],
                        "fileSize": 1
                    }],
                    "dependencies": { "minecraft": "1.21.1", "fabric-loader": "0.16.9" }
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap();
        archive.finish().unwrap();
        let manager = DownloadManager::new(paths.downloads_state(), 3).unwrap();
        assert!(
            import_mrpack_with_download_manager(
                &paths,
                &pack,
                Some("broken"),
                &manager,
                &mut |_, _| {},
                |_| {},
            )
            .is_err()
        );
        assert!(!paths.profile_dir("broken").exists());
        assert!(!paths.profiles.join(".broken.staging").exists());
        let _ = fs::remove_dir_all(paths.profiles.parent().unwrap());
    }
}
