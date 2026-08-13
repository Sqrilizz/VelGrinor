use crate::curseforge::{CurseForgeClient, get_sha1_hash};
use crate::download::{DownloadManager, DownloadRequest, DownloadSnapshot};
use crate::paths::Paths;
use crate::profile::{ContentRef, Files, Loader, Profile, Runtime};
use crate::store::{ContentKind, store_content};
use crate::util::atomic_write;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::fs;
use std::io::{Read, Seek};
use std::path::{Component, Path, PathBuf};
use zip::ZipArchive;

const MAX_OVERRIDE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_OVERRIDE_FILES: usize = 100_000;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeManifest {
    pub manifest_type: String,
    pub manifest_version: u32,
    pub name: String,
    pub version: String,
    pub minecraft: CurseForgeMinecraft,
    pub files: Vec<CurseForgeManifestFile>,
    #[serde(default = "default_overrides")]
    pub overrides: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeMinecraft {
    pub version: String,
    #[serde(default)]
    pub mod_loaders: Vec<CurseForgeManifestLoader>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CurseForgeManifestLoader {
    pub id: String,
    #[serde(default)]
    pub primary: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeManifestFile {
    pub project_id: u32,
    pub file_id: u32,
    pub required: bool,
}

pub fn import_curseforge_zip(
    paths: &Paths,
    archive_path: &Path,
    requested_profile_id: Option<&str>,
    api_key: &str,
    progress: impl FnMut(u8, String),
) -> Result<Profile> {
    let manager = DownloadManager::new(paths.downloads_state(), 3)?;
    import_curseforge_zip_managed(
        paths,
        archive_path,
        requested_profile_id,
        api_key,
        &manager,
        progress,
        |_| {},
    )
}

pub fn import_curseforge_zip_managed(
    paths: &Paths,
    archive_path: &Path,
    requested_profile_id: Option<&str>,
    api_key: &str,
    download_manager: &DownloadManager,
    mut progress: impl FnMut(u8, String),
    download_progress: impl Fn(DownloadSnapshot) + Send + Sync,
) -> Result<Profile> {
    let file = fs::File::open(archive_path)?;
    let mut archive = ZipArchive::new(file).context("invalid CurseForge ZIP")?;
    let manifest = read_manifest(&mut archive)?;
    validate_manifest(&manifest)?;
    validate_archive(&mut archive, &manifest.overrides)?;
    let profile_id = requested_profile_id
        .map(str::to_string)
        .unwrap_or_else(|| slugify(&manifest.name));
    if profile_id.is_empty() {
        bail!("profile id cannot be empty");
    }
    if paths.is_profile_present(&profile_id) {
        bail!("profile already exists: {profile_id}");
    }
    let loader = resolve_loader(&manifest.minecraft.mod_loaders)?;
    let staging = paths.profiles.join(format!(".{profile_id}.staging"));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(staging.join("overrides"))?;
    let mut staging_guard = StagingGuard::new(staging.clone());
    let client = CurseForgeClient::new(api_key);
    let mut mods = Vec::new();
    let mut pending = Vec::new();
    for (index, reference) in manifest.files.iter().enumerate() {
        progress(
            ((index * 90) / manifest.files.len().max(1)) as u8,
            format!("Downloading {}/{}", index + 1, manifest.files.len()),
        );
        let project = client.get_mod(reference.project_id)?;
        let remote = client.get_file(reference.project_id, reference.file_id)?;
        if remote.id != reference.file_id || remote.mod_id != reference.project_id {
            bail!("CurseForge returned a different project/file pair")
        }
        validate_file_compatibility(
            &remote.game_versions,
            &manifest.minecraft.version,
            loader.as_ref(),
        )?;
        let download = paths
            .cache_downloads
            .join(format!("cf-{}-{}", reference.project_id, reference.file_id));
        let url = remote
            .download_url
            .clone()
            .context("CurseForge file distribution is disabled")?;
        let mut request = DownloadRequest::new(url, &download);
        request.sha1 = get_sha1_hash(&remote).map(str::to_string);
        request.label = Some(remote.file_name.clone());
        request.group = Some(format!("curseforge-pack-{profile_id}"));
        pending.push((reference.clone(), project, remote, download, request));
    }
    download_manager.run_requests(
        pending.iter().map(|item| item.4.clone()).collect(),
        download_progress,
    )?;
    for (reference, project, remote, download, _) in pending {
        let stored = store_content(
            paths,
            ContentKind::Mod,
            &download,
            remote.download_url.clone(),
            Some(remote.file_name.clone()),
        )?;
        mods.push(ContentRef {
            name: project.name,
            hash: format!("sha256:{}", stored.hash),
            version: Some(remote.display_name),
            source: remote.download_url,
            file_name: Some(remote.file_name),
            platform: Some("curseforge".to_string()),
            project_id: Some(reference.project_id.to_string()),
            version_id: Some(reference.file_id.to_string()),
            enabled: reference.required,
            pinned: false,
        });
        let _ = fs::remove_file(download);
    }
    extract_overrides(
        &mut archive,
        &manifest.overrides,
        &staging.join("overrides"),
    )?;
    let profile = Profile {
        id: profile_id.clone(),
        mc_version: manifest.minecraft.version,
        loader,
        mods,
        resourcepacks: Vec::new(),
        shaderpacks: Vec::new(),
        runtime: Runtime::default(),
        files: Files::default(),
    };
    atomic_write(
        &staging.join("profile.json"),
        serde_json::to_vec_pretty(&profile)?,
    )?;
    fs::rename(&staging, paths.profile_dir(&profile_id))?;
    staging_guard.keep();
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

fn read_manifest<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<CurseForgeManifest> {
    let mut file = archive
        .by_name("manifest.json")
        .context("manifest.json not found")?;
    let mut data = String::new();
    file.read_to_string(&mut data)?;
    Ok(serde_json::from_str(&data)?)
}

pub fn is_curseforge_zip(path: &Path) -> bool {
    fs::File::open(path)
        .ok()
        .and_then(|file| ZipArchive::new(file).ok())
        .and_then(|mut archive| archive.by_name("manifest.json").ok().map(|_| ()))
        .is_some()
}

fn validate_manifest(manifest: &CurseForgeManifest) -> Result<()> {
    if !manifest
        .manifest_type
        .eq_ignore_ascii_case("minecraftModpack")
    {
        bail!("unsupported CurseForge manifest type");
    }
    if manifest.manifest_version != 1 {
        bail!("unsupported CurseForge manifest version");
    }
    if manifest.minecraft.version.trim().is_empty() {
        bail!("Minecraft version is missing");
    }
    if manifest
        .files
        .iter()
        .any(|file| file.project_id == 0 || file.file_id == 0)
    {
        bail!("invalid CurseForge projectID/fileID");
    }
    Ok(())
}

fn resolve_loader(loaders: &[CurseForgeManifestLoader]) -> Result<Option<Loader>> {
    let selected = loaders
        .iter()
        .find(|loader| loader.primary)
        .or_else(|| loaders.first());
    let Some(selected) = selected else {
        return Ok(None);
    };
    let (kind, version) = selected
        .id
        .split_once('-')
        .context("invalid CurseForge loader identifier")?;
    let kind = match kind.to_ascii_lowercase().as_str() {
        "forge" => "forge",
        "neoforge" => "neoforge",
        "fabric" | "fabricloader" => "fabric",
        "quilt" | "quiltloader" => "quilt",
        other => bail!("unsupported CurseForge loader: {other}"),
    };
    Ok(Some(Loader {
        loader_type: kind.to_string(),
        version: version.to_string(),
    }))
}

fn validate_file_compatibility(
    versions: &[String],
    minecraft: &str,
    loader: Option<&Loader>,
) -> Result<()> {
    if !versions.iter().any(|version| version == minecraft) {
        bail!("CurseForge file is incompatible with Minecraft {minecraft}");
    }
    if let Some(loader) = loader {
        let compatible = versions
            .iter()
            .any(|value| value.eq_ignore_ascii_case(&loader.loader_type));
        if !compatible {
            bail!(
                "CurseForge file is incompatible with {}",
                loader.loader_type
            );
        }
    }
    Ok(())
}

fn validate_archive<R: Read + Seek>(archive: &mut ZipArchive<R>, overrides: &str) -> Result<()> {
    let prefix = format!("{}/", overrides.trim_matches('/'));
    let mut count = 0;
    let mut size = 0_u64;
    for index in 0..archive.len() {
        let file = archive.by_index(index)?;
        if !file.name().starts_with(&prefix) || file.is_dir() {
            continue;
        }
        sanitize_path(file.name().trim_start_matches(&prefix))?;
        count += 1;
        size = size.saturating_add(file.size());
        if count > MAX_OVERRIDE_FILES || size > MAX_OVERRIDE_BYTES {
            bail!("CurseForge overrides exceed extraction limits");
        }
    }
    Ok(())
}

fn extract_overrides<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    overrides: &str,
    target: &Path,
) -> Result<()> {
    let prefix = format!("{}/", overrides.trim_matches('/'));
    for index in 0..archive.len() {
        let mut file = archive.by_index(index)?;
        if !file.name().starts_with(&prefix) || file.is_dir() {
            continue;
        }
        let relative = sanitize_path(file.name().trim_start_matches(&prefix))?;
        let destination = target.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = fs::File::create(destination)?;
        std::io::copy(&mut file, &mut output)?;
    }
    Ok(())
}

fn sanitize_path(value: &str) -> Result<PathBuf> {
    let mut result = PathBuf::new();
    for component in Path::new(value).components() {
        match component {
            Component::Normal(value) => result.push(value),
            Component::CurDir => {}
            _ => bail!("unsafe path in CurseForge archive: {value}"),
        }
    }
    if result.as_os_str().is_empty() {
        bail!("empty path in CurseForge archive");
    }
    Ok(result)
}

fn slugify(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn default_overrides() -> String {
    "overrides".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_all_requested_loaders() {
        for (input, expected) in [
            ("forge-47.3.0", "forge"),
            ("neoforge-21.1.0", "neoforge"),
            ("fabric-0.16.0", "fabric"),
            ("quilt-0.27.0", "quilt"),
        ] {
            let loader = resolve_loader(&[CurseForgeManifestLoader {
                id: input.to_string(),
                primary: true,
            }])
            .unwrap()
            .unwrap();
            assert_eq!(loader.loader_type, expected);
        }
    }

    #[test]
    fn rejects_unsafe_paths() {
        assert!(sanitize_path("../escape.txt").is_err());
        assert!(sanitize_path("/absolute.txt").is_err());
        assert!(sanitize_path("config/safe.txt").is_ok());
    }
}
