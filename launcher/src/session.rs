use crate::util::{atomic_write, now_epoch_secs};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

static SESSION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub pid: u32,
    pub profile_id: String,
    pub java: String,
    pub ram: Option<String>,
    pub gpu: Option<String>,
    pub started_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    #[serde(flatten)]
    pub session: SessionInfo,
    pub ended_at: u64,
    pub duration_seconds: u64,
    pub exit_code: Option<i32>,
    pub crash_report: Option<PathBuf>,
}

#[derive(Clone)]
pub struct LaunchManager {
    active_path: PathBuf,
    records_path: PathBuf,
    state: Arc<Mutex<LaunchState>>,
}

struct LaunchState {
    active: Option<SessionInfo>,
    child: Option<Child>,
}

impl LaunchManager {
    pub fn new(data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref();
        fs::create_dir_all(data_dir)?;
        let active_path = data_dir.join("active-session.json");
        let records_path = data_dir.join("sessions.json");
        let active = if active_path.exists() {
            let session: SessionInfo = serde_json::from_slice(&fs::read(&active_path)?)?;
            process_matches(&session).then_some(session)
        } else {
            None
        };
        if active.is_none() && active_path.exists() {
            fs::remove_file(&active_path)?;
        }
        Ok(Self {
            active_path,
            records_path,
            state: Arc::new(Mutex::new(LaunchState {
                active,
                child: None,
            })),
        })
    }

    pub fn get_active_session(&self) -> Option<SessionInfo> {
        let mut state = self.state.lock().unwrap();
        if let Some(child) = state.child.as_mut()
            && child.try_wait().ok().flatten().is_some()
        {
            state.active = None;
            state.child = None;
            let _ = fs::remove_file(&self.active_path);
        }
        state.active.clone()
    }

    pub fn launch<F>(
        &self,
        mut command: Command,
        profile_id: String,
        java: String,
        ram: Option<String>,
        gpu: Option<String>,
        finished: F,
    ) -> Result<SessionInfo>
    where
        F: Fn(SessionRecord) + Send + 'static,
    {
        if self.get_active_session().is_some() {
            bail!("Minecraft is already running");
        }
        let child = command.spawn().context("failed to start Minecraft")?;
        let started_at = now_epoch_secs();
        let sequence = SESSION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let session = SessionInfo {
            session_id: format!("{started_at:x}-{sequence:x}-{:x}", child.id()),
            pid: child.id(),
            profile_id,
            java,
            ram,
            gpu,
            started_at,
        };
        atomic_write(&self.active_path, serde_json::to_vec_pretty(&session)?)?;
        {
            let mut state = self.state.lock().unwrap();
            state.active = Some(session.clone());
            state.child = Some(child);
        }
        let manager = self.clone();
        std::thread::spawn(move || {
            loop {
                let status = {
                    let mut state = manager.state.lock().unwrap();
                    state
                        .child
                        .as_mut()
                        .and_then(|child| child.try_wait().ok().flatten())
                };
                if let Some(status) = status {
                    let ended_at = now_epoch_secs();
                    let record = SessionRecord {
                        session: session.clone(),
                        ended_at,
                        duration_seconds: ended_at.saturating_sub(session.started_at),
                        exit_code: status.code(),
                        crash_report: None,
                    };
                    let _ = manager.finish(&record);
                    finished(record);
                    break;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        });
        Ok(self.get_active_session().unwrap())
    }

    pub fn stop(&self, session_id: &str) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        let active = state
            .active
            .clone()
            .context("no active Minecraft session")?;
        if active.session_id != session_id {
            bail!("session ID does not match the active process");
        }
        if let Some(child) = state.child.as_mut() {
            child.kill().context("failed to stop Minecraft")?;
            return Ok(());
        }
        if !process_matches(&active) {
            bail!("the restored process no longer matches this session");
        }
        stop_pid(active.pid)
    }

    pub fn records(&self) -> Result<Vec<SessionRecord>> {
        if !self.records_path.exists() {
            return Ok(Vec::new());
        }
        Ok(serde_json::from_slice(&fs::read(&self.records_path)?)?)
    }

    pub fn attach_crash_report(
        &self,
        session_id: &str,
        crash_report: impl Into<PathBuf>,
    ) -> Result<()> {
        let mut records = self.records()?;
        let record = records
            .iter_mut()
            .find(|record| record.session.session_id == session_id)
            .context("session record not found")?;
        record.crash_report = Some(crash_report.into());
        atomic_write(&self.records_path, serde_json::to_vec_pretty(&records)?)
    }

    fn finish(&self, record: &SessionRecord) -> Result<()> {
        {
            let mut state = self.state.lock().unwrap();
            if state
                .active
                .as_ref()
                .map(|session| session.session_id.as_str())
                == Some(&record.session.session_id)
            {
                state.active = None;
                state.child = None;
            }
        }
        if self.active_path.exists() {
            fs::remove_file(&self.active_path)?;
        }
        let mut records = self.records().unwrap_or_default();
        records.insert(0, record.clone());
        records.truncate(100);
        atomic_write(&self.records_path, serde_json::to_vec_pretty(&records)?)
    }
}

#[cfg(unix)]
fn process_matches(session: &SessionInfo) -> bool {
    let cmdline = fs::read(format!("/proc/{}/cmdline", session.pid)).unwrap_or_default();
    let java_name = Path::new(&session.java)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("java");
    !cmdline.is_empty() && String::from_utf8_lossy(&cmdline).contains(java_name)
}

#[cfg(windows)]
fn process_matches(_: &SessionInfo) -> bool {
    false
}

#[cfg(not(any(unix, windows)))]
fn process_matches(_: &SessionInfo) -> bool {
    false
}

#[cfg(unix)]
fn stop_pid(pid: u32) -> Result<()> {
    let status = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()?;
    if !status.success() {
        bail!("failed to stop restored Minecraft process");
    }
    Ok(())
}

#[cfg(windows)]
fn stop_pid(pid: u32) -> Result<()> {
    let status = Command::new("taskkill")
        .args(["/PID", &pid.to_string()])
        .status()?;
    if !status.success() {
        bail!("failed to stop restored Minecraft process");
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn stop_pid(_: u32) -> Result<()> {
    bail!("stopping restored sessions is unsupported")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Instant;

    fn root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "velgrinor-session-{name}-{}-{}",
            std::process::id(),
            SESSION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn rejects_double_launch_and_requires_exact_session_id_to_stop() {
        let root = root("exclusive");
        let manager = LaunchManager::new(&root).unwrap();
        let (sender, receiver) = mpsc::channel();
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 10"]);
        let session = manager
            .launch(
                command,
                "profile".into(),
                "sh".into(),
                Some("2G".into()),
                None,
                move |record| sender.send(record).unwrap(),
            )
            .unwrap();
        let mut second = Command::new("sh");
        second.args(["-c", "exit 0"]);
        assert!(
            manager
                .launch(second, "profile".into(), "sh".into(), None, None, |_| {})
                .is_err()
        );
        assert!(manager.stop("wrong-session").is_err());
        manager.stop(&session.session_id).unwrap();
        let record = receiver.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(record.session.session_id, session.session_id);
        assert!(manager.get_active_session().is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn records_normal_and_crash_exit_and_attaches_report() {
        let root = root("records");
        let manager = LaunchManager::new(&root).unwrap();
        for code in [0, 7] {
            let (sender, receiver) = mpsc::channel();
            let mut command = Command::new("sh");
            command.args(["-c", &format!("exit {code}")]);
            let session = manager
                .launch(
                    command,
                    "profile".into(),
                    "sh".into(),
                    None,
                    None,
                    move |record| sender.send(record).unwrap(),
                )
                .unwrap();
            let record = receiver.recv_timeout(Duration::from_secs(3)).unwrap();
            assert_eq!(record.exit_code, Some(code));
            if code != 0 {
                manager
                    .attach_crash_report(&session.session_id, root.join("crash.txt"))
                    .unwrap();
            }
            let deadline = Instant::now() + Duration::from_secs(1);
            while manager.get_active_session().is_some() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let records = manager.records().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].exit_code, Some(7));
        assert_eq!(records[0].crash_report, Some(root.join("crash.txt")));
        assert_eq!(records[1].exit_code, Some(0));
        let _ = fs::remove_dir_all(root);
    }
}
