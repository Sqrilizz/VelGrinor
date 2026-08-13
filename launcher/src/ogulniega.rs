use anyhow::{Context, Result, bail};
use reqwest::Url;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path};
use std::time::Duration;

pub const SITE_URL: &str = "https://ogulniega.com";
pub const CATALOG_URL: &str = "https://ogulniega.com/files/launcher.json";
pub const DEFAULT_PROFILE_URL: &str = "https://ogulniega.com/files/default_profile.zip";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OgulniegaCatalog {
    pub versions: Vec<OgulniegaPack>,
    #[serde(default)]
    pub selected_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OgulniegaPack {
    pub name: String,
    pub minecraft_version: String,
    pub fabric_version: String,
    pub loader_name: String,
    #[serde(default)]
    pub java_name: Option<String>,
    #[serde(default)]
    pub jvm_args: Vec<String>,
}

impl OgulniegaPack {
    pub fn loader_version(&self) -> Result<&str> {
        self.loader_name
            .rsplit_once('-')
            .map(|(_, version)| version)
            .filter(|version| !version.is_empty())
            .context("invalid Ogulniega loader name")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OgulniegaVersionManifest {
    #[serde(default)]
    pub mods: Vec<OgulniegaMod>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OgulniegaMod {
    pub id: String,
    pub name: String,
    pub url: String,
    pub sha512: String,
}

pub fn fetch_catalog() -> Result<OgulniegaCatalog> {
    client()?
        .get(CATALOG_URL)
        .send()
        .context("failed to load Ogulniega catalog")?
        .error_for_status()
        .context("Ogulniega catalog returned an error")?
        .json()
        .context("failed to parse Ogulniega catalog")
}

pub fn fetch_version_manifest(pack_name: &str) -> Result<OgulniegaVersionManifest> {
    validate_pack_name(pack_name)?;
    let url = format!("{SITE_URL}/files/client_versions/{pack_name}.json");
    let manifest: OgulniegaVersionManifest = client()?
        .get(url)
        .send()
        .context("failed to load Ogulniega build manifest")?
        .error_for_status()
        .context("Ogulniega build manifest returned an error")?
        .json()
        .context("failed to parse Ogulniega build manifest")?;
    if manifest.mods.is_empty() {
        bail!("Ogulniega build contains no mods");
    }
    for item in &manifest.mods {
        validate_mod(item)?;
    }
    Ok(manifest)
}

pub fn extract_default_profile(archive_path: &Path, destination: &Path) -> Result<()> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file).context("invalid Ogulniega profile archive")?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let Some(relative) = entry.enclosed_name() else {
            bail!("unsafe path in Ogulniega profile archive");
        };
        if entry.is_symlink() {
            bail!("symlinks are not allowed in Ogulniega profile archive");
        }
        let output = destination.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&output)?;
        } else {
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut target = std::fs::File::create(&output)?;
            std::io::copy(&mut entry, &mut target)?;
        }
    }
    Ok(())
}

fn client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("VelGrinor/0.1 Ogulniega integration")
        .build()
        .context("failed to create Ogulniega client")
}

fn validate_pack_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 80
        || !name
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'.' | b'-' | b'_'))
    {
        bail!("invalid Ogulniega build name");
    }
    Ok(())
}

fn validate_mod(item: &OgulniegaMod) -> Result<()> {
    let path = Path::new(&item.name);
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) || path.file_name().and_then(|value| value.to_str()) != Some(item.name.as_str())
    {
        bail!("unsafe mod filename in Ogulniega manifest: {}", item.name);
    }
    if item.sha512.len() != 128 || !item.sha512.bytes().all(|value| value.is_ascii_hexdigit()) {
        bail!("invalid SHA-512 for Ogulniega mod: {}", item.name);
    }
    let url = Url::parse(&item.url).context("invalid Ogulniega mod URL")?;
    let allowed_host = matches!(url.host_str(), Some("ogulniega.com" | "cdn.modrinth.com"));
    if url.scheme() != "https" || !allowed_host || url.username() != "" || url.password().is_some()
    {
        bail!("untrusted Ogulniega mod URL: {}", item.url);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_pack_names() {
        assert!(validate_pack_name("1.21.1-sodium").is_ok());
        assert!(validate_pack_name("../escape").is_err());
    }

    #[test]
    fn rejects_untrusted_mods() {
        let mut item = OgulniegaMod {
            id: "mod".to_string(),
            name: "mod.jar".to_string(),
            url: "https://ogulniega.com/files/mod.jar".to_string(),
            sha512: "00".repeat(64),
        };
        assert!(validate_mod(&item).is_ok());
        item.url = "https://example.com/mod.jar".to_string();
        assert!(validate_mod(&item).is_err());
        item.url = "https://ogulniega.com/files/mod.jar".to_string();
        item.name = "../mod.jar".to_string();
        assert!(validate_mod(&item).is_err());
    }

    #[test]
    fn reads_loader_version_from_loader_name() {
        let pack = OgulniegaPack {
            name: "1.20.1-sodium".to_string(),
            minecraft_version: "1.20.1".to_string(),
            fabric_version: "0.19.3".to_string(),
            loader_name: "fabric-1.20.1-0.17.3".to_string(),
            java_name: None,
            jvm_args: Vec::new(),
        };
        assert_eq!(pack.loader_version().unwrap(), "0.17.3");
    }
}
