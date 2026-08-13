use crate::paths::Paths;
use crate::profile::load_profile;
use crate::util::{atomic_write, copy_dir_all, now_epoch_secs};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static SNAPSHOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub id: String,
    pub profile_id: String,
    pub created_at: u64,
    pub automatic: bool,
    pub reason: String,
    pub size: u64,
    pub changes: Vec<String>,
}

pub fn create_snapshot(
    paths: &Paths,
    profile_id: &str,
    automatic: bool,
    reason: impl Into<String>,
) -> Result<SnapshotInfo> {
    let profile = load_profile(paths, profile_id)?;
    let sequence = SNAPSHOT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let created_at = now_epoch_secs();
    let id = format!("{created_at}-{sequence}");
    let root = snapshot_root(paths, profile_id);
    fs::create_dir_all(&root)?;
    let staging = root.join(format!(".{id}.staging"));
    let destination = root.join(&id);
    fs::create_dir_all(&staging)?;
    atomic_write(
        &staging.join("profile.json"),
        serde_json::to_vec_pretty(&profile)?,
    )?;
    let overrides = paths.profile_overrides(profile_id);
    if overrides.exists() {
        copy_dir_all(&overrides, &staging.join("overrides"))?;
    }
    let size = directory_size(&staging)?;
    let info = SnapshotInfo {
        id: id.clone(),
        profile_id: profile_id.to_string(),
        created_at,
        automatic,
        reason: reason.into(),
        size,
        changes: vec![
            "profile manifest".to_string(),
            "overrides and config".to_string(),
        ],
    };
    atomic_write(
        &staging.join("snapshot.json"),
        serde_json::to_vec_pretty(&info)?,
    )?;
    fs::rename(&staging, &destination)?;
    Ok(info)
}

pub fn list_snapshots(paths: &Paths, profile_id: &str) -> Result<Vec<SnapshotInfo>> {
    let root = snapshot_root(paths, profile_id);
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut snapshots = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let metadata = entry.path().join("snapshot.json");
        if !metadata.exists() {
            continue;
        }
        let data = fs::read(&metadata)?;
        snapshots.push(
            serde_json::from_slice(&data)
                .with_context(|| format!("failed to parse {}", metadata.display()))?,
        );
    }
    snapshots.sort_by_key(|snapshot: &SnapshotInfo| std::cmp::Reverse(snapshot.created_at));
    Ok(snapshots)
}

pub fn prune_automatic_snapshots(paths: &Paths, profile_id: &str, retention: usize) -> Result<()> {
    let snapshots = list_snapshots(paths, profile_id)?;
    for snapshot in snapshots
        .into_iter()
        .filter(|snapshot| snapshot.automatic)
        .skip(retention)
    {
        fs::remove_dir_all(snapshot_root(paths, profile_id).join(snapshot.id))?;
    }
    Ok(())
}

pub fn restore_snapshot(
    paths: &Paths,
    profile_id: &str,
    snapshot_id: &str,
) -> Result<SnapshotInfo> {
    let snapshot = snapshot_root(paths, profile_id).join(snapshot_id);
    let metadata: SnapshotInfo =
        serde_json::from_slice(&fs::read(snapshot.join("snapshot.json"))?)?;
    let profile_data = fs::read(snapshot.join("profile.json"))?;
    let _: crate::profile::Profile = serde_json::from_slice(&profile_data)?;
    let rollback = create_snapshot(
        paths,
        profile_id,
        true,
        format!("before rollback to {snapshot_id}"),
    )?;
    atomic_write(&paths.profile_json(profile_id), profile_data)?;

    let overrides = paths.profile_overrides(profile_id);
    let staged = paths
        .profile_dir(profile_id)
        .join(format!(".overrides-{snapshot_id}.staging"));
    let backup = paths
        .profile_dir(profile_id)
        .join(format!(".overrides-{snapshot_id}.backup"));
    if staged.exists() {
        fs::remove_dir_all(&staged)?;
    }
    if snapshot.join("overrides").exists() {
        copy_dir_all(&snapshot.join("overrides"), &staged)?;
    } else {
        fs::create_dir_all(&staged)?;
    }
    if overrides.exists() {
        fs::rename(&overrides, &backup)?;
    }
    if let Err(error) = fs::rename(&staged, &overrides) {
        if backup.exists() {
            let _ = fs::rename(&backup, &overrides);
        }
        return Err(error.into());
    }
    if backup.exists() {
        fs::remove_dir_all(backup)?;
    }
    Ok(SnapshotInfo {
        reason: format!(
            "restored {}; rollback snapshot {}",
            metadata.id, rollback.id
        ),
        ..metadata
    })
}

fn snapshot_root(paths: &Paths, profile_id: &str) -> PathBuf {
    paths
        .profiles
        .parent()
        .unwrap_or(Path::new("."))
        .join("snapshots")
        .join(profile_id)
}

fn directory_size(path: &Path) -> Result<u64> {
    let mut size = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        size += if metadata.is_dir() {
            directory_size(&entry.path())?
        } else {
            metadata.len()
        };
    }
    Ok(size)
}
