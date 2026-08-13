use crate::instance::materialize_instance;
use crate::java::{detect_installations, get_required_java_version, is_java_compatible};
use crate::paths::Paths;
use crate::profile::{ContentRef, Profile, save_profile};
use crate::store::store_from_url;
use crate::store::{ContentKind, content_store_path};
use crate::util::atomic_write;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticIssue {
    pub code: String,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub evidence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<DiagnosticFix>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DiagnosticFix {
    RestoreFile {
        kind: ContentKind,
        content: ContentRef,
    },
    AddDependency {
        content: ContentRef,
    },
    SelectJava {
        path: String,
    },
    DisableConflict {
        kind: ContentKind,
        hash: String,
    },
    RebuildInstance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticReport {
    pub profile_id: String,
    pub blocking: bool,
    pub issues: Vec<DiagnosticIssue>,
}

pub fn diagnose_profile(paths: &Paths, profile: &Profile) -> Result<DiagnosticReport> {
    let mut issues = Vec::new();
    inspect_content(paths, &profile.mods, ContentKind::Mod, &mut issues)?;
    inspect_content(
        paths,
        &profile.resourcepacks,
        ContentKind::ResourcePack,
        &mut issues,
    )?;
    inspect_content(
        paths,
        &profile.shaderpacks,
        ContentKind::ShaderPack,
        &mut issues,
    )?;
    inspect_duplicates(profile, &mut issues);
    inspect_mod_dependencies(paths, profile, &mut issues)?;
    inspect_loader(profile, &mut issues);
    inspect_java(profile, &mut issues);
    inspect_memory(profile, &mut issues);
    inspect_instance(paths, profile, &mut issues)?;
    inspect_permissions(paths, profile, &mut issues);
    let blocking = issues
        .iter()
        .any(|issue| issue.severity == DiagnosticSeverity::Error);
    Ok(DiagnosticReport {
        profile_id: profile.id.clone(),
        blocking,
        issues,
    })
}

pub fn apply_fix(paths: &Paths, profile: &mut Profile, fix: &DiagnosticFix) -> Result<()> {
    match fix {
        DiagnosticFix::RestoreFile { kind, content } => {
            let source = content
                .source
                .as_deref()
                .context("content has no recovery source")?;
            let (download, _) = store_from_url(paths, source)?;
            let expected = content
                .hash
                .strip_prefix("sha256:")
                .unwrap_or(&content.hash);
            let actual = sha256_file(&download)?;
            if !actual.eq_ignore_ascii_case(expected) {
                bail!("recovered file failed SHA-256 validation");
            }
            atomic_write(
                &content_store_path(paths, *kind, &content.hash),
                fs::read(download)?,
            )?;
        }
        DiagnosticFix::AddDependency { content } => {
            if !content_store_path(paths, ContentKind::Mod, &content.hash).exists() {
                bail!("dependency file is not present in the local library")
            }
            if !profile.mods.iter().any(|item| item.hash == content.hash) {
                profile.mods.push(content.clone());
            }
            save_profile(paths, profile)?;
        }
        DiagnosticFix::SelectJava { path } => {
            profile.runtime.java = Some(path.clone());
            save_profile(paths, profile)?;
        }
        DiagnosticFix::DisableConflict { kind, hash } => {
            let items = match kind {
                ContentKind::Mod => &mut profile.mods,
                ContentKind::ResourcePack => &mut profile.resourcepacks,
                ContentKind::ShaderPack => &mut profile.shaderpacks,
                _ => bail!("unsupported content kind for conflict fix"),
            };
            let item = items
                .iter_mut()
                .find(|item| item.hash == *hash)
                .context("conflicting content not found")?;
            item.enabled = false;
            save_profile(paths, profile)?;
        }
        DiagnosticFix::RebuildInstance => {
            materialize_instance(paths, profile)?;
        }
    }
    Ok(())
}

fn inspect_content(
    paths: &Paths,
    items: &[ContentRef],
    kind: ContentKind,
    issues: &mut Vec<DiagnosticIssue>,
) -> Result<()> {
    for item in items.iter().filter(|item| item.enabled) {
        let path = content_store_path(paths, kind, &item.hash);
        if !path.exists() {
            issues.push(issue(
                "missing_file",
                DiagnosticSeverity::Error,
                format!("{} is missing", item.name),
                path.display().to_string(),
                item.source.as_ref().map(|_| DiagnosticFix::RestoreFile {
                    kind,
                    content: item.clone(),
                }),
            ));
            continue;
        }
        let expected = item.hash.strip_prefix("sha256:").unwrap_or(&item.hash);
        if expected.len() == 64 {
            let actual = sha256_file(&path)?;
            if !actual.eq_ignore_ascii_case(expected) {
                issues.push(issue(
                    "corrupt_file",
                    DiagnosticSeverity::Error,
                    format!("{} failed SHA-256 validation", item.name),
                    format!("expected {expected}, got {actual}"),
                    item.source.as_ref().map(|_| DiagnosticFix::RestoreFile {
                        kind,
                        content: item.clone(),
                    }),
                ));
            }
        }
    }
    Ok(())
}

fn inspect_duplicates(profile: &Profile, issues: &mut Vec<DiagnosticIssue>) {
    let mut names = HashMap::<String, Vec<&ContentRef>>::new();
    for item in profile.mods.iter().filter(|item| item.enabled) {
        let key = item
            .file_name
            .as_deref()
            .unwrap_or(&item.name)
            .to_ascii_lowercase();
        names.entry(key).or_default().push(item);
    }
    for (name, items) in names.into_iter().filter(|(_, items)| items.len() > 1) {
        issues.push(issue(
            "duplicate_mod",
            DiagnosticSeverity::Error,
            format!("duplicate enabled mod: {name}"),
            items
                .iter()
                .map(|item| item.hash.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            Some(DiagnosticFix::DisableConflict {
                kind: ContentKind::Mod,
                hash: items[1].hash.clone(),
            }),
        ));
    }
}

#[derive(Default)]
struct ModMetadata {
    ids: Vec<String>,
    versions: HashMap<String, String>,
    required: Vec<String>,
    incompatible: Vec<String>,
    requirements: HashMap<String, Vec<String>>,
    minecraft: Vec<String>,
    loader: Vec<String>,
}

fn inspect_mod_dependencies(
    paths: &Paths,
    profile: &Profile,
    issues: &mut Vec<DiagnosticIssue>,
) -> Result<()> {
    let mut metadata = Vec::new();
    let mut installed = HashMap::<String, Option<String>>::new();
    for item in profile.mods.iter().filter(|item| item.enabled) {
        installed.insert(item.name.to_ascii_lowercase(), item.version.clone());
        if let Some(project) = &item.project_id {
            installed.insert(project.to_ascii_lowercase(), item.version.clone());
        }
        let path = content_store_path(paths, ContentKind::Mod, &item.hash);
        if !path.exists() {
            continue;
        }
        if let Ok(found) = read_mod_metadata(&path) {
            for id in &found.ids {
                installed.insert(
                    id.to_ascii_lowercase(),
                    found
                        .versions
                        .get(id)
                        .cloned()
                        .or_else(|| item.version.clone()),
                );
            }
            metadata.push((item, found));
        }
    }
    for (item, found) in metadata {
        for dependency in &found.required {
            let normalized = dependency.to_ascii_lowercase();
            if ![
                "minecraft",
                "java",
                "fabricloader",
                "forge",
                "neoforge",
                "quilt_loader",
            ]
            .contains(&normalized.as_str())
                && !installed.contains_key(&normalized)
            {
                issues.push(issue(
                    "missing_dependency",
                    DiagnosticSeverity::Error,
                    format!("{} requires {}", item.name, dependency),
                    format!("mod metadata dependency: {dependency}"),
                    None,
                ));
            }
        }
        for incompatible in &found.incompatible {
            let normalized = incompatible.to_ascii_lowercase();
            if installed.get(&normalized).is_some_and(|version| {
                relation_matches(found.requirements.get(&normalized), version.as_deref())
            }) {
                issues.push(issue(
                    "incompatible_mod",
                    DiagnosticSeverity::Error,
                    format!("{} is incompatible with {}", item.name, incompatible),
                    format!("mod metadata conflict: {incompatible}"),
                    Some(DiagnosticFix::DisableConflict {
                        kind: ContentKind::Mod,
                        hash: item.hash.clone(),
                    }),
                ));
            }
        }
        if !found.minecraft.is_empty()
            && found.minecraft.iter().all(|requirement| {
                is_exact_version(requirement) && requirement != &profile.mc_version
            })
        {
            issues.push(issue(
                "minecraft_incompatible",
                DiagnosticSeverity::Error,
                format!(
                    "{} does not support Minecraft {}",
                    item.name, profile.mc_version
                ),
                found.minecraft.join(", "),
                Some(DiagnosticFix::DisableConflict {
                    kind: ContentKind::Mod,
                    hash: item.hash.clone(),
                }),
            ));
        }
        if let Some(loader) = &profile.loader
            && !found.loader.is_empty()
            && found.loader.iter().all(|required| {
                semver::VersionReq::parse(required)
                    .ok()
                    .zip(semver::Version::parse(&loader.version).ok())
                    .is_some_and(|(requirement, version)| !requirement.matches(&version))
            })
        {
            issues.push(issue(
                "loader_incompatible",
                DiagnosticSeverity::Error,
                format!(
                    "{} requires a different {} version",
                    item.name, loader.loader_type
                ),
                found.loader.join(", "),
                Some(DiagnosticFix::DisableConflict {
                    kind: ContentKind::Mod,
                    hash: item.hash.clone(),
                }),
            ));
        }
    }
    Ok(())
}

fn read_mod_metadata(path: &Path) -> Result<ModMetadata> {
    let file = fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    if let Some(data) = read_zip_entry(&mut archive, "fabric.mod.json")? {
        let mut metadata = parse_fabric_metadata(&data)?;
        merge_nested_metadata(&mut archive, &mut metadata)?;
        return Ok(metadata);
    }
    if let Some(data) = read_zip_entry(&mut archive, "quilt.mod.json")? {
        return parse_quilt_metadata(&data);
    }
    for name in ["META-INF/neoforge.mods.toml", "META-INF/mods.toml"] {
        if let Some(data) = read_zip_entry(&mut archive, name)? {
            return Ok(parse_forge_metadata(&data));
        }
    }
    Ok(ModMetadata::default())
}

fn read_zip_entry<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<Option<String>> {
    let Ok(mut entry) = archive.by_name(name) else {
        return Ok(None);
    };
    let mut data = String::new();
    entry.read_to_string(&mut data)?;
    Ok(Some(data))
}

fn merge_nested_metadata<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    metadata: &mut ModMetadata,
) -> Result<()> {
    merge_nested_metadata_at(archive, metadata, 0)
}

fn merge_nested_metadata_at<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    metadata: &mut ModMetadata,
    depth: usize,
) -> Result<()> {
    if depth >= 8 {
        return Ok(());
    }
    let names = (0..archive.len())
        .filter_map(|index| {
            archive.by_index(index).ok().and_then(|entry| {
                let name = entry.name().to_string();
                (name.starts_with("META-INF/jars/") && name.ends_with(".jar")).then_some(name)
            })
        })
        .collect::<Vec<_>>();
    for name in names {
        let mut entry = archive.by_name(&name)?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        drop(entry);
        let mut nested = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
        if let Some(data) = read_zip_entry(&mut nested, "fabric.mod.json")? {
            let mut child = parse_fabric_metadata(&data)?;
            merge_nested_metadata_at(&mut nested, &mut child, depth + 1)?;
            merge_metadata(metadata, child);
        }
    }
    Ok(())
}

fn merge_metadata(target: &mut ModMetadata, source: ModMetadata) {
    target.ids.extend(source.ids);
    target.versions.extend(source.versions);
    target.required.extend(source.required);
    target.incompatible.extend(source.incompatible);
    target.requirements.extend(source.requirements);
    target.minecraft.extend(source.minecraft);
    target.loader.extend(source.loader);
}

fn parse_fabric_metadata(data: &str) -> Result<ModMetadata> {
    let value: serde_json::Value = serde_json::from_str(data)?;
    let mut metadata = ModMetadata::default();
    if let Some(id) = value.get("id").and_then(|value| value.as_str()) {
        metadata.ids.push(id.to_string());
        if let Some(version) = value.get("version").and_then(|value| value.as_str()) {
            metadata
                .versions
                .insert(id.to_string(), version.to_string());
        }
    }
    if let Some(provides) = value.get("provides").and_then(|value| value.as_array()) {
        metadata.ids.extend(
            provides
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string)),
        );
    }
    collect_relation(
        value.get("depends"),
        &mut metadata.required,
        &mut metadata.requirements,
    );
    collect_relation(
        value.get("breaks"),
        &mut metadata.incompatible,
        &mut metadata.requirements,
    );
    collect_relation(
        value.get("conflicts"),
        &mut metadata.incompatible,
        &mut metadata.requirements,
    );
    if let Some(depends) = value.get("depends").and_then(|value| value.as_object()) {
        collect_requirements(depends.get("minecraft"), &mut metadata.minecraft);
        collect_requirements(depends.get("fabricloader"), &mut metadata.loader);
    }
    Ok(metadata)
}

fn parse_quilt_metadata(data: &str) -> Result<ModMetadata> {
    let value: serde_json::Value = serde_json::from_str(data)?;
    let loader = value
        .get("quilt_loader")
        .and_then(|value| value.as_object())
        .context("quilt_loader metadata is missing")?;
    let mut metadata = ModMetadata::default();
    if let Some(id) = loader.get("id").and_then(|value| value.as_str()) {
        metadata.ids.push(id.to_string());
    }
    collect_quilt_relations(loader.get("depends"), &mut metadata.required);
    collect_quilt_relations(loader.get("breaks"), &mut metadata.incompatible);
    Ok(metadata)
}

fn collect_quilt_relations(value: Option<&serde_json::Value>, target: &mut Vec<String>) {
    let Some(values) = value.and_then(|value| value.as_array()) else {
        return;
    };
    for value in values {
        if let Some(id) = value.as_str().or_else(|| {
            value
                .as_object()
                .and_then(|object| object.get("id"))
                .and_then(|id| id.as_str())
        }) {
            target.push(id.to_string());
        }
    }
}

fn parse_forge_metadata(data: &str) -> ModMetadata {
    let mut metadata = ModMetadata::default();
    let mut dependency_section = false;
    let mut dependency_id: Option<String> = None;
    let mut dependency_required = true;
    let mut dependency_incompatible = false;
    let mut dependency_range: Option<String> = None;
    for line in data.lines().map(str::trim) {
        if line.starts_with("[[dependencies.") {
            flush_forge_dependency(
                &mut metadata,
                dependency_id.take(),
                dependency_required,
                dependency_incompatible,
                dependency_range.take(),
            );
            dependency_section = true;
            dependency_required = true;
            dependency_incompatible = false;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches(['"', '\'']);
        if !dependency_section && key == "modId" {
            metadata.ids.push(value.to_string());
        } else if dependency_section {
            match key {
                "modId" => dependency_id = Some(value.to_string()),
                "mandatory" => dependency_required = value.eq_ignore_ascii_case("true"),
                "type" => {
                    dependency_required = value.eq_ignore_ascii_case("required");
                    dependency_incompatible = value.eq_ignore_ascii_case("incompatible");
                }
                "versionRange" => dependency_range = Some(value.to_string()),
                _ => {}
            }
        }
    }
    flush_forge_dependency(
        &mut metadata,
        dependency_id,
        dependency_required,
        dependency_incompatible,
        dependency_range,
    );
    metadata
}

fn flush_forge_dependency(
    metadata: &mut ModMetadata,
    id: Option<String>,
    required: bool,
    incompatible: bool,
    version_range: Option<String>,
) {
    let Some(id) = id else { return };
    if id == "minecraft" {
        if let Some(version) = version_range.and_then(|value| exact_maven_version(&value)) {
            metadata.minecraft.push(version);
        }
    } else if id == "forge" || id == "neoforge" {
        if let Some(version) = version_range.and_then(|value| exact_maven_version(&value)) {
            metadata.loader.push(version);
        }
    } else if incompatible {
        metadata.incompatible.push(id);
    } else if required {
        metadata.required.push(id);
    }
}

fn exact_maven_version(value: &str) -> Option<String> {
    let value = value.strip_prefix('[')?.strip_suffix(']')?;
    (!value.contains(',')).then(|| value.to_string())
}

fn collect_relation(
    value: Option<&serde_json::Value>,
    target: &mut Vec<String>,
    requirements: &mut HashMap<String, Vec<String>>,
) {
    if let Some(object) = value.and_then(|value| value.as_object()) {
        for (id, value) in object {
            target.push(id.clone());
            let mut versions = Vec::new();
            collect_requirements(Some(value), &mut versions);
            requirements.insert(id.to_ascii_lowercase(), versions);
        }
    }
}

fn collect_requirements(value: Option<&serde_json::Value>, target: &mut Vec<String>) {
    match value {
        Some(serde_json::Value::String(value)) => target.push(value.clone()),
        Some(serde_json::Value::Array(values)) => target.extend(
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string)),
        ),
        _ => {}
    }
}

fn is_exact_version(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_digit() || character == '.')
}

fn relation_matches(requirements: Option<&Vec<String>>, installed: Option<&str>) -> bool {
    let Some(requirements) = requirements else {
        return true;
    };
    if requirements.is_empty() || requirements.iter().any(|value| value == "*") {
        return true;
    }
    let Some(installed) = installed.and_then(|value| semver::Version::parse(value).ok()) else {
        return false;
    };
    requirements.iter().any(|requirement| {
        semver::VersionReq::parse(requirement)
            .map(|requirement| requirement.matches(&installed))
            .unwrap_or(false)
    })
}

fn inspect_loader(profile: &Profile, issues: &mut Vec<DiagnosticIssue>) {
    if let Some(loader) = &profile.loader {
        let supported = ["fabric", "forge", "neoforge", "quilt"];
        if !supported.contains(&loader.loader_type.to_ascii_lowercase().as_str()) {
            issues.push(issue(
                "unsupported_loader",
                DiagnosticSeverity::Error,
                "unsupported mod loader".to_string(),
                loader.loader_type.clone(),
                None,
            ));
        }
    } else if !profile.mods.is_empty() {
        issues.push(issue(
            "missing_loader",
            DiagnosticSeverity::Error,
            "mods require a mod loader".to_string(),
            "profile loader is empty".to_string(),
            None,
        ));
    }
    if !profile.shaderpacks.is_empty() && profile.mods.is_empty() {
        issues.push(issue(
            "missing_shader_loader",
            DiagnosticSeverity::Warning,
            "shaderpacks may require Iris or Oculus".to_string(),
            "no enabled mods provide a shader loader".to_string(),
            None,
        ));
    }
}

fn inspect_java(profile: &Profile, issues: &mut Vec<DiagnosticIssue>) {
    let required = get_required_java_version(&profile.mc_version);
    if let Some(path) = &profile.runtime.java {
        if !Path::new(path).exists() && path != "java" {
            issues.push(issue(
                "java_missing",
                DiagnosticSeverity::Error,
                "configured Java executable is missing".to_string(),
                path.clone(),
                detect_installations()
                    .into_iter()
                    .find(|java| {
                        java.major
                            .map(|major| is_java_compatible(major, &profile.mc_version))
                            .unwrap_or(false)
                    })
                    .map(|java| DiagnosticFix::SelectJava { path: java.path }),
            ));
        }
    } else if !detect_installations().iter().any(|java| {
        java.major
            .map(|major| is_java_compatible(major, &profile.mc_version))
            .unwrap_or(false)
    }) {
        issues.push(issue(
            "java_incompatible",
            DiagnosticSeverity::Error,
            format!("Java {required} or newer is required"),
            profile.mc_version.clone(),
            None,
        ));
    }
}

fn inspect_memory(profile: &Profile, issues: &mut Vec<DiagnosticIssue>) {
    if let Some(memory) = &profile.runtime.memory {
        let normalized = memory.trim().to_ascii_lowercase();
        let parsed = normalized
            .strip_suffix('g')
            .and_then(|value| value.parse::<u64>().ok())
            .map(|value| value * 1024)
            .or_else(|| {
                normalized
                    .strip_suffix('m')
                    .and_then(|value| value.parse::<u64>().ok())
            });
        if parsed.is_none() || parsed == Some(0) {
            issues.push(issue(
                "invalid_memory",
                DiagnosticSeverity::Error,
                "invalid RAM setting".to_string(),
                memory.clone(),
                None,
            ));
        } else if parsed.unwrap_or(0) < 1024 {
            issues.push(issue(
                "low_memory",
                DiagnosticSeverity::Warning,
                "less than 1 GiB of RAM is configured".to_string(),
                memory.clone(),
                None,
            ));
        }
    }
}

fn inspect_instance(
    paths: &Paths,
    profile: &Profile,
    issues: &mut Vec<DiagnosticIssue>,
) -> Result<()> {
    let instance = paths.instance_dir(&profile.id);
    if !instance.exists() {
        return Ok(());
    }
    for directory in ["mods", "resourcepacks", "shaderpacks"] {
        let path = instance.join(directory);
        if !path.exists() {
            continue;
        }
        for entry in fs::read_dir(&path)? {
            let entry = entry?;
            if entry.file_type()?.is_symlink() {
                issues.push(issue(
                    "legacy_symlink",
                    DiagnosticSeverity::Error,
                    "legacy symlink must be rebuilt".to_string(),
                    entry.path().display().to_string(),
                    Some(DiagnosticFix::RebuildInstance),
                ));
            }
        }
    }
    Ok(())
}

fn inspect_permissions(paths: &Paths, profile: &Profile, issues: &mut Vec<DiagnosticIssue>) {
    let mut checked = HashSet::new();
    for path in [
        &paths.profiles,
        &paths.instances,
        &paths.cache_downloads,
        &paths.profile_dir(&profile.id),
    ] {
        if checked.insert(path.to_path_buf())
            && path.exists()
            && path
                .metadata()
                .map(|value| value.permissions().readonly())
                .unwrap_or(true)
        {
            issues.push(issue(
                "directory_readonly",
                DiagnosticSeverity::Error,
                "launcher directory is read-only".to_string(),
                path.display().to_string(),
                None,
            ));
        }
    }
}

fn issue(
    code: &str,
    severity: DiagnosticSeverity,
    message: String,
    evidence: String,
    fix: Option<DiagnosticFix>,
) -> DiagnosticIssue {
    DiagnosticIssue {
        code: code.to_string(),
        severity,
        message,
        evidence,
        fix,
    }
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fabric_dependencies_and_conflicts() {
        let metadata = parse_fabric_metadata(
            r#"{"id":"demo","depends":{"fabricloader":">=0.16","minecraft":"1.21.1","required-lib":"*"},"breaks":{"bad-lib":"*"}}"#,
        )
        .unwrap();
        assert_eq!(metadata.ids, ["demo"]);
        assert!(metadata.required.contains(&"required-lib".to_string()));
        assert!(metadata.incompatible.contains(&"bad-lib".to_string()));
        assert_eq!(metadata.minecraft, ["1.21.1"]);
        assert!(relation_matches(
            metadata.requirements.get("bad-lib"),
            Some("0.9.0")
        ));
    }

    #[test]
    fn conditional_conflicts_only_match_affected_versions() {
        let metadata = parse_fabric_metadata(
            r#"{"id":"demo","version":"1.0.0","breaks":{"sodium":"<=0.6.6"}}"#,
        )
        .unwrap();
        assert!(relation_matches(
            metadata.requirements.get("sodium"),
            Some("0.6.6")
        ));
        assert!(!relation_matches(
            metadata.requirements.get("sodium"),
            Some("0.9.1")
        ));
    }

    #[test]
    fn parses_quilt_dependency_objects() {
        let metadata = parse_quilt_metadata(
            r#"{"quilt_loader":{"id":"demo","depends":[{"id":"required-lib","versions":"*"}],"breaks":["bad-lib"]}}"#,
        )
        .unwrap();
        assert_eq!(metadata.ids, ["demo"]);
        assert_eq!(metadata.required, ["required-lib"]);
        assert_eq!(metadata.incompatible, ["bad-lib"]);
    }

    #[test]
    fn parses_forge_and_neoforge_dependency_relations() {
        let metadata = parse_forge_metadata(
            r#"
modId="demo"
[[dependencies.demo]]
modId="minecraft"
mandatory=true
versionRange="[1.21.1]"
[[dependencies.demo]]
modId="required-lib"
type="required"
[[dependencies.demo]]
modId="bad-lib"
type="incompatible"
"#,
        );
        assert_eq!(metadata.ids, ["demo"]);
        assert_eq!(metadata.minecraft, ["1.21.1"]);
        assert!(metadata.required.contains(&"required-lib".to_string()));
        assert!(metadata.incompatible.contains(&"bad-lib".to_string()));
    }
}
