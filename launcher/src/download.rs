use crate::util::{atomic_write, replace_file};
use anyhow::{Context, Result, bail};
use reqwest::StatusCode;
use reqwest::blocking::Client;
use reqwest::header::{CONTENT_LENGTH, CONTENT_RANGE, RANGE};
use serde::{Deserialize, Serialize};
use sha1::{Digest as _, Sha1};
use sha2::{Sha256, Sha512};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static DOWNLOAD_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadRequest {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub mirrors: Vec<String>,
    pub destination: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha512: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

impl DownloadRequest {
    pub fn new(url: impl Into<String>, destination: impl Into<PathBuf>) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let sequence = DOWNLOAD_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        Self {
            id: format!("{now:x}-{sequence:x}"),
            url: url.into(),
            mirrors: Vec::new(),
            destination: destination.into(),
            expected_size: None,
            sha1: None,
            sha256: None,
            sha512: None,
            group: None,
            label: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStatus {
    Queued,
    Running,
    Paused,
    Cancelled,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadSnapshot {
    pub request: DownloadRequest,
    pub status: DownloadStatus,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub speed_bytes_per_second: u64,
    pub eta_seconds: Option<u64>,
    pub attempts: u8,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DownloadGroupSnapshot {
    pub group: Option<String>,
    pub files_completed: usize,
    pub files_total: usize,
    pub bytes_downloaded: u64,
    pub bytes_total: Option<u64>,
    pub speed_bytes_per_second: u64,
    pub eta_seconds: Option<u64>,
    pub downloads: Vec<DownloadSnapshot>,
}

#[derive(Clone)]
pub struct DownloadManager {
    state_path: PathBuf,
    concurrency: usize,
    client: Client,
    downloads: Arc<Mutex<Vec<DownloadSnapshot>>>,
    running: Arc<AtomicBool>,
}

impl DownloadManager {
    pub fn new(state_path: impl Into<PathBuf>, concurrency: usize) -> Result<Self> {
        let state_path = state_path.into();
        let mut downloads = if state_path.exists() {
            let data = fs::read(&state_path).with_context(|| {
                format!("failed to read download queue: {}", state_path.display())
            })?;
            serde_json::from_slice::<Vec<DownloadSnapshot>>(&data).with_context(|| {
                format!("failed to parse download queue: {}", state_path.display())
            })?
        } else {
            Vec::new()
        };
        for download in &mut downloads {
            if matches!(
                download.status,
                DownloadStatus::Running | DownloadStatus::Queued
            ) {
                download.status = DownloadStatus::Paused;
                download.speed_bytes_per_second = 0;
                download.eta_seconds = None;
            }
        }
        let manager = Self {
            state_path,
            concurrency: concurrency.clamp(1, 16),
            client: Client::builder().timeout(Duration::from_secs(30)).build()?,
            downloads: Arc::new(Mutex::new(downloads)),
            running: Arc::new(AtomicBool::new(false)),
        };
        manager.persist()?;
        Ok(manager)
    }

    pub fn enqueue(&self, request: DownloadRequest) -> Result<String> {
        let id = request.id.clone();
        let downloaded = part_path(&request.destination)
            .metadata()
            .map(|value| value.len())
            .unwrap_or(0);
        self.downloads.lock().unwrap().push(DownloadSnapshot {
            total: request.expected_size,
            request,
            status: DownloadStatus::Queued,
            downloaded,
            speed_bytes_per_second: 0,
            eta_seconds: None,
            attempts: 0,
            error: None,
        });
        self.persist()?;
        Ok(id)
    }

    pub fn pause(&self, id: &str) -> Result<()> {
        self.set_status(id, DownloadStatus::Paused)
    }

    pub fn cancel(&self, id: &str) -> Result<()> {
        let destination = {
            let mut downloads = self.downloads.lock().unwrap();
            let download = downloads
                .iter_mut()
                .find(|item| item.request.id == id)
                .context("download not found")?;
            download.status = DownloadStatus::Cancelled;
            download.request.destination.clone()
        };
        let _ = fs::remove_file(part_path(&destination));
        self.persist()
    }

    pub fn retry(&self, id: &str) -> Result<()> {
        let mut downloads = self.downloads.lock().unwrap();
        let download = downloads
            .iter_mut()
            .find(|item| item.request.id == id)
            .context("download not found")?;
        download.status = DownloadStatus::Queued;
        download.error = None;
        download.attempts = 0;
        drop(downloads);
        self.persist()
    }

    pub fn resume(&self, id: &str) -> Result<()> {
        self.set_status(id, DownloadStatus::Queued)
    }

    pub fn snapshots(&self) -> Vec<DownloadSnapshot> {
        self.downloads.lock().unwrap().clone()
    }

    pub fn group_snapshot(&self, group: Option<&str>) -> DownloadGroupSnapshot {
        let downloads = self
            .downloads
            .lock()
            .unwrap()
            .iter()
            .filter(|item| item.request.group.as_deref() == group)
            .cloned()
            .collect::<Vec<_>>();
        let bytes_total = downloads
            .iter()
            .try_fold(0_u64, |sum, item| item.total.map(|value| sum + value));
        let speed = downloads
            .iter()
            .map(|item| item.speed_bytes_per_second)
            .sum::<u64>();
        let remaining = bytes_total
            .map(|total| total.saturating_sub(downloads.iter().map(|item| item.downloaded).sum()));
        DownloadGroupSnapshot {
            group: group.map(str::to_string),
            files_completed: downloads
                .iter()
                .filter(|item| item.status == DownloadStatus::Completed)
                .count(),
            files_total: downloads.len(),
            bytes_downloaded: downloads.iter().map(|item| item.downloaded).sum(),
            bytes_total,
            speed_bytes_per_second: speed,
            eta_seconds: remaining.filter(|_| speed > 0).map(|value| value / speed),
            downloads,
        }
    }

    pub fn run_pending<F>(&self, progress: F) -> Result<()>
    where
        F: Fn(DownloadSnapshot) + Send + Sync,
    {
        while self
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        struct RunningGuard(Arc<AtomicBool>);
        impl Drop for RunningGuard {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _guard = RunningGuard(self.running.clone());
        let callback = &progress;
        std::thread::scope(|scope| {
            let mut workers = Vec::new();
            for _ in 0..self.concurrency {
                let manager = self.clone();
                workers.push(scope.spawn(move || {
                    loop {
                        let id = manager
                            .downloads
                            .lock()
                            .unwrap()
                            .iter()
                            .find(|item| item.status == DownloadStatus::Queued)
                            .map(|item| item.request.id.clone());
                        let Some(id) = id else { break };
                        manager.execute(&id, callback);
                    }
                }));
            }
            for worker in workers {
                worker
                    .join()
                    .map_err(|_| anyhow::anyhow!("download worker panicked"))?;
            }
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(())
    }

    pub fn run_requests<F>(
        &self,
        requests: Vec<DownloadRequest>,
        progress: F,
    ) -> Result<Vec<DownloadSnapshot>>
    where
        F: Fn(DownloadSnapshot) + Send + Sync,
    {
        let ids = requests
            .into_iter()
            .map(|request| self.enqueue(request))
            .collect::<Result<Vec<_>>>()?;
        self.run_pending(progress)?;
        let snapshots = self.downloads.lock().unwrap();
        let completed = ids
            .iter()
            .map(|id| {
                snapshots
                    .iter()
                    .find(|item| item.request.id == *id)
                    .cloned()
                    .context("download disappeared from queue")
            })
            .collect::<Result<Vec<_>>>()?;
        if let Some(failed) = completed
            .iter()
            .find(|item| item.status != DownloadStatus::Completed)
        {
            bail!(
                "download {} did not complete: {}",
                failed
                    .request
                    .label
                    .as_deref()
                    .unwrap_or(&failed.request.url),
                failed.error.as_deref().unwrap_or(match failed.status {
                    DownloadStatus::Paused => "paused",
                    DownloadStatus::Cancelled => "cancelled",
                    DownloadStatus::Queued => "queued",
                    DownloadStatus::Running => "running",
                    DownloadStatus::Completed => "completed",
                    DownloadStatus::Failed => "failed",
                })
            );
        }
        Ok(completed)
    }

    fn execute(&self, id: &str, progress: &dyn Fn(DownloadSnapshot)) {
        let request = {
            let mut downloads = self.downloads.lock().unwrap();
            let Some(download) = downloads.iter_mut().find(|item| item.request.id == id) else {
                return;
            };
            if download.status != DownloadStatus::Queued {
                return;
            }
            download.status = DownloadStatus::Running;
            download.request.clone()
        };
        let _ = self.persist();
        let mut last_error = None;
        let mut urls = vec![request.url.clone()];
        urls.extend(request.mirrors.clone());
        for attempt in 1..=3_u8 {
            let url = &urls[(attempt as usize - 1) % urls.len()];
            self.update(id, |download| download.attempts = attempt, progress);
            match self.download_once(id, &request, url, progress) {
                Ok(true) => return,
                Ok(false) => return,
                Err(error) => last_error = Some(format!("{error:#}")),
            }
        }
        self.update(
            id,
            |download| {
                download.status = DownloadStatus::Failed;
                download.error = last_error;
                download.speed_bytes_per_second = 0;
                download.eta_seconds = None;
            },
            progress,
        );
    }

    fn download_once(
        &self,
        id: &str,
        request: &DownloadRequest,
        url: &str,
        progress: &dyn Fn(DownloadSnapshot),
    ) -> Result<bool> {
        let part = part_path(&request.destination);
        if let Some(parent) = part.parent() {
            fs::create_dir_all(parent)?;
        }
        let existing = part.metadata().map(|value| value.len()).unwrap_or(0);
        let mut builder = self.client.get(url);
        if existing > 0 {
            builder = builder.header(RANGE, format!("bytes={existing}-"));
        }
        let mut response = builder
            .send()
            .with_context(|| format!("failed to download {url}"))?;
        if response.status() == StatusCode::RANGE_NOT_SATISFIABLE {
            if request.expected_size == Some(existing) {
                self.finish(id, request, &part, progress)?;
                return Ok(true);
            }
            fs::write(&part, [])?;
            bail!("server rejected resume range");
        }
        if !response.status().is_success() {
            bail!("download returned HTTP {}", response.status());
        }
        let resumed = response.status() == StatusCode::PARTIAL_CONTENT;
        let base = if resumed { existing } else { 0 };
        let response_length = response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        let total = content_range_total(
            response
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|value| value.to_str().ok()),
        )
        .or_else(|| response_length.map(|value| value + base))
        .or(request.expected_size);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(resumed)
            .truncate(!resumed)
            .open(&part)?;
        let started = Instant::now();
        let mut downloaded = base;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            match self.status(id) {
                Some(DownloadStatus::Paused) => return Ok(false),
                Some(DownloadStatus::Cancelled) | None => {
                    drop(file);
                    let _ = fs::remove_file(&part);
                    return Ok(false);
                }
                _ => {}
            }
            let read = response.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            file.write_all(&buffer[..read])?;
            downloaded += read as u64;
            let elapsed = started.elapsed().as_secs_f64().max(0.001);
            let speed = ((downloaded - base) as f64 / elapsed) as u64;
            self.update(
                id,
                |item| {
                    item.downloaded = downloaded;
                    item.total = total;
                    item.speed_bytes_per_second = speed;
                    item.eta_seconds = total
                        .filter(|_| speed > 0)
                        .map(|value| value.saturating_sub(downloaded) / speed);
                },
                progress,
            );
        }
        file.sync_all()?;
        self.finish(id, request, &part, progress)?;
        Ok(true)
    }

    fn finish(
        &self,
        id: &str,
        request: &DownloadRequest,
        part: &Path,
        progress: &dyn Fn(DownloadSnapshot),
    ) -> Result<()> {
        let size = part.metadata()?.len();
        if let Some(expected) = request.expected_size
            && size != expected
        {
            let _ = fs::remove_file(part);
            bail!("size mismatch: expected {expected}, got {size}");
        }
        if let Err(error) = verify_hashes(
            part,
            request.sha1.as_deref(),
            request.sha256.as_deref(),
            request.sha512.as_deref(),
        ) {
            let _ = fs::remove_file(part);
            return Err(error);
        }
        replace_file(part, &request.destination)?;
        self.update(
            id,
            |item| {
                item.status = DownloadStatus::Completed;
                item.downloaded = size;
                item.total = Some(size);
                item.speed_bytes_per_second = 0;
                item.eta_seconds = Some(0);
                item.error = None;
            },
            progress,
        );
        Ok(())
    }

    fn update(
        &self,
        id: &str,
        update: impl FnOnce(&mut DownloadSnapshot),
        progress: &dyn Fn(DownloadSnapshot),
    ) {
        let snapshot = {
            let mut downloads = self.downloads.lock().unwrap();
            let Some(download) = downloads.iter_mut().find(|item| item.request.id == id) else {
                return;
            };
            update(download);
            download.clone()
        };
        let _ = self.persist();
        progress(snapshot);
    }

    fn status(&self, id: &str) -> Option<DownloadStatus> {
        self.downloads
            .lock()
            .unwrap()
            .iter()
            .find(|item| item.request.id == id)
            .map(|item| item.status)
    }

    fn set_status(&self, id: &str, status: DownloadStatus) -> Result<()> {
        let mut downloads = self.downloads.lock().unwrap();
        let download = downloads
            .iter_mut()
            .find(|item| item.request.id == id)
            .context("download not found")?;
        download.status = status;
        download.error = None;
        drop(downloads);
        self.persist()
    }

    fn persist(&self) -> Result<()> {
        let downloads = self.downloads.lock().unwrap();
        let unfinished = downloads
            .iter()
            .filter(|download| {
                !matches!(
                    download.status,
                    DownloadStatus::Completed | DownloadStatus::Cancelled
                )
            })
            .collect::<Vec<_>>();
        let data = serde_json::to_vec_pretty(&unfinished)?;
        atomic_write(&self.state_path, data)
    }
}

fn part_path(destination: &Path) -> PathBuf {
    let mut name = destination.as_os_str().to_os_string();
    name.push(".part");
    PathBuf::from(name)
}

fn content_range_total(value: Option<&str>) -> Option<u64> {
    value?.split('/').nth(1)?.parse().ok()
}

fn verify_hashes(
    path: &Path,
    sha1: Option<&str>,
    sha256: Option<&str>,
    sha512: Option<&str>,
) -> Result<()> {
    if sha1.is_none() && sha256.is_none() && sha512.is_none() {
        return Ok(());
    }
    let mut file = fs::File::open(path)?;
    let mut sha1_hasher = Sha1::new();
    let mut sha256_hasher = Sha256::new();
    let mut sha512_hasher = Sha512::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        sha1_hasher.update(&buffer[..read]);
        sha256_hasher.update(&buffer[..read]);
        sha512_hasher.update(&buffer[..read]);
    }
    if let Some(expected) = sha1
        && hex::encode(sha1_hasher.finalize()) != expected.to_ascii_lowercase()
    {
        bail!("SHA-1 checksum mismatch");
    }
    if let Some(expected) = sha256
        && hex::encode(sha256_hasher.finalize()) != expected.to_ascii_lowercase()
    {
        bail!("SHA-256 checksum mismatch");
    }
    if let Some(expected) = sha512
        && hex::encode(sha512_hasher.finalize()) != expected.to_ascii_lowercase()
    {
        bail!("SHA-512 checksum mismatch");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::thread;

    static TEST_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    struct TestResponse {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        chunk_size: usize,
        chunk_delay: Duration,
    }

    impl TestResponse {
        fn ok(body: impl Into<Vec<u8>>) -> Self {
            Self {
                status: 200,
                headers: Vec::new(),
                body: body.into(),
                chunk_size: usize::MAX,
                chunk_delay: Duration::ZERO,
            }
        }
    }

    struct TestServer {
        url: String,
        stop: Arc<AtomicBool>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl TestServer {
        fn start(handler: impl Fn(String) -> TestResponse + Send + Sync + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let thread_stop = stop.clone();
            let handler = Arc::new(handler);
            let thread = thread::spawn(move || {
                while !thread_stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let handler = handler.clone();
                            thread::spawn(move || serve(stream, handler));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(_) => break,
                    }
                }
            });
            Self {
                url: format!("http://{address}/file"),
                stop,
                thread: Some(thread),
            }
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(thread) = self.thread.take() {
                thread.join().unwrap();
            }
        }
    }

    fn serve(stream: TcpStream, handler: Arc<impl Fn(String) -> TestResponse>) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request = String::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                break;
            }
            request.push_str(&line);
        }
        let response = handler(request);
        let reason = match response.status {
            200 => "OK",
            206 => "Partial Content",
            416 => "Range Not Satisfiable",
            _ => "Server Error",
        };
        let mut stream = stream;
        write!(
            stream,
            "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
            response.status,
            reason,
            response.body.len()
        )
        .unwrap();
        for (name, value) in response.headers {
            write!(stream, "{name}: {value}\r\n").unwrap();
        }
        write!(stream, "\r\n").unwrap();
        for chunk in response.body.chunks(response.chunk_size.max(1)) {
            if stream.write_all(chunk).is_err() {
                break;
            }
            let _ = stream.flush();
            thread::sleep(response.chunk_delay);
        }
    }

    fn test_root(name: &str) -> PathBuf {
        let sequence = TEST_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "velgrinor-{name}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn manager(root: &Path, concurrency: usize) -> DownloadManager {
        DownloadManager::new(root.join("downloads.json"), concurrency).unwrap()
    }

    #[test]
    fn running_downloads_restore_paused() {
        let root = test_root("restore");
        let state = root.join("downloads.json");
        fs::create_dir_all(&root).unwrap();
        let request = DownloadRequest::new("https://example.invalid/file", root.join("file"));
        let snapshot = DownloadSnapshot {
            request,
            status: DownloadStatus::Running,
            downloaded: 10,
            total: Some(20),
            speed_bytes_per_second: 5,
            eta_seconds: Some(2),
            attempts: 1,
            error: None,
        };
        atomic_write(&state, serde_json::to_vec(&vec![snapshot]).unwrap()).unwrap();
        let manager = DownloadManager::new(&state, 3).unwrap();
        assert_eq!(manager.snapshots()[0].status, DownloadStatus::Paused);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn persisted_queue_contains_only_unfinished_downloads() {
        let root = test_root("unfinished-only");
        let server = TestServer::start(|_| TestResponse::ok(b"done".to_vec()));
        let download_manager = manager(&root, 1);
        download_manager
            .run_requests(
                vec![DownloadRequest::new(&server.url, root.join("completed"))],
                |_| {},
            )
            .unwrap();
        let paused_id = download_manager
            .enqueue(DownloadRequest::new(
                "https://example.invalid/paused",
                root.join("paused"),
            ))
            .unwrap();
        download_manager.pause(&paused_id).unwrap();
        let restored = manager(&root, 1).snapshots();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].request.id, paused_id);
        assert_eq!(restored[0].status, DownloadStatus::Paused);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_checksum_mismatch() {
        let root = test_root("hash-unit");
        let file = root.join("file");
        fs::write(&file, b"content").unwrap();
        assert!(verify_hashes(&file, None, Some("bad"), None).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resumes_with_206() {
        let root = test_root("resume-206");
        let server = TestServer::start(|request| {
            assert!(request.to_ascii_lowercase().contains("range: bytes=3-"));
            let mut response = TestResponse::ok(b"def".to_vec());
            response.status = 206;
            response
                .headers
                .push(("Content-Range".into(), "bytes 3-5/6".into()));
            response
        });
        let destination = root.join("file");
        fs::write(part_path(&destination), b"abc").unwrap();
        manager(&root, 3)
            .run_requests(
                vec![DownloadRequest::new(&server.url, &destination)],
                |_| {},
            )
            .unwrap();
        assert_eq!(fs::read(destination).unwrap(), b"abcdef");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restarts_when_server_ignores_range() {
        let root = test_root("range-200");
        let server = TestServer::start(|request| {
            assert!(request.to_ascii_lowercase().contains("range: bytes=3-"));
            TestResponse::ok(b"abcdef".to_vec())
        });
        let destination = root.join("file");
        fs::write(part_path(&destination), b"abc").unwrap();
        manager(&root, 3)
            .run_requests(
                vec![DownloadRequest::new(&server.url, &destination)],
                |_| {},
            )
            .unwrap();
        assert_eq!(fs::read(destination).unwrap(), b"abcdef");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn completes_matching_part_after_416() {
        let root = test_root("range-416");
        let server = TestServer::start(|_| TestResponse {
            status: 416,
            headers: Vec::new(),
            body: Vec::new(),
            chunk_size: usize::MAX,
            chunk_delay: Duration::ZERO,
        });
        let destination = root.join("file");
        fs::write(part_path(&destination), b"abc").unwrap();
        let mut request = DownloadRequest::new(&server.url, &destination);
        request.expected_size = Some(3);
        manager(&root, 3)
            .run_requests(vec![request], |_| {})
            .unwrap();
        assert_eq!(fs::read(destination).unwrap(), b"abc");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn retries_with_mirror() {
        let root = test_root("mirror");
        let failures = Arc::new(AtomicUsize::new(0));
        let failure_count = failures.clone();
        let primary = TestServer::start(move |_| {
            failure_count.fetch_add(1, AtomicOrdering::SeqCst);
            TestResponse {
                status: 500,
                headers: Vec::new(),
                body: Vec::new(),
                chunk_size: usize::MAX,
                chunk_delay: Duration::ZERO,
            }
        });
        let mirror = TestServer::start(|_| TestResponse::ok(b"mirror".to_vec()));
        let destination = root.join("file");
        let mut request = DownloadRequest::new(&primary.url, &destination);
        request.mirrors.push(mirror.url.clone());
        let snapshots = manager(&root, 3)
            .run_requests(vec![request], |_| {})
            .unwrap();
        assert_eq!(snapshots[0].attempts, 2);
        assert_eq!(failures.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(fs::read(destination).unwrap(), b"mirror");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn checksum_mismatch_fails_without_replacing_destination() {
        let root = test_root("checksum");
        let server = TestServer::start(|_| TestResponse::ok(b"invalid".to_vec()));
        let destination = root.join("file");
        fs::write(&destination, b"original").unwrap();
        let mut request = DownloadRequest::new(&server.url, &destination);
        request.sha256 = Some("00".repeat(32));
        let result = manager(&root, 3).run_requests(vec![request], |_| {});
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("checksum mismatch")
        );
        assert_eq!(fs::read(destination).unwrap(), b"original");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pause_resume_and_cancel_are_cooperative() {
        let root = test_root("controls");
        let body = vec![7_u8; 256 * 1024];
        let server_body = body.clone();
        let server = TestServer::start(move |request| {
            let start = request
                .lines()
                .find(|line| line.to_ascii_lowercase().starts_with("range:"))
                .and_then(|line| line.split("bytes=").nth(1))
                .and_then(|value| value.trim_end_matches('-').parse::<usize>().ok())
                .unwrap_or(0);
            let mut response = TestResponse::ok(server_body[start..].to_vec());
            if start > 0 {
                response.status = 206;
                response.headers.push((
                    "Content-Range".into(),
                    format!(
                        "bytes {start}-{}/{}",
                        server_body.len() - 1,
                        server_body.len()
                    ),
                ));
            }
            response.chunk_size = 4096;
            response.chunk_delay = Duration::from_millis(4);
            response
        });
        let download_manager = manager(&root, 1);
        let paused_destination = root.join("paused");
        let paused_id = download_manager
            .enqueue(DownloadRequest::new(&server.url, &paused_destination))
            .unwrap();
        let runner = download_manager.clone();
        let thread = thread::spawn(move || runner.run_pending(|_| {}).unwrap());
        while part_path(&paused_destination)
            .metadata()
            .map(|metadata| metadata.len())
            .unwrap_or(0)
            == 0
        {
            thread::sleep(Duration::from_millis(2));
        }
        download_manager.pause(&paused_id).unwrap();
        thread.join().unwrap();
        assert_eq!(
            download_manager.status(&paused_id),
            Some(DownloadStatus::Paused)
        );
        download_manager.resume(&paused_id).unwrap();
        download_manager.run_pending(|_| {}).unwrap();
        assert_eq!(fs::read(&paused_destination).unwrap(), body);

        let cancelled_destination = root.join("cancelled");
        let cancelled_id = download_manager
            .enqueue(DownloadRequest::new(&server.url, &cancelled_destination))
            .unwrap();
        let runner = download_manager.clone();
        let thread = thread::spawn(move || runner.run_pending(|_| {}).unwrap());
        while part_path(&cancelled_destination)
            .metadata()
            .map(|metadata| metadata.len())
            .unwrap_or(0)
            == 0
        {
            thread::sleep(Duration::from_millis(2));
        }
        download_manager.cancel(&cancelled_id).unwrap();
        thread.join().unwrap();
        assert!(!part_path(&cancelled_destination).exists());
        assert!(!cancelled_destination.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn limits_concurrency_to_three() {
        let root = test_root("concurrency");
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let handler_active = active.clone();
        let handler_maximum = maximum.clone();
        let server = TestServer::start(move |_| {
            let current = handler_active.fetch_add(1, AtomicOrdering::SeqCst) + 1;
            handler_maximum.fetch_max(current, AtomicOrdering::SeqCst);
            thread::sleep(Duration::from_millis(40));
            handler_active.fetch_sub(1, AtomicOrdering::SeqCst);
            TestResponse::ok(vec![1_u8; 32])
        });
        let requests = (0..9)
            .map(|index| DownloadRequest::new(&server.url, root.join(format!("file-{index}"))))
            .collect();
        manager(&root, 3).run_requests(requests, |_| {}).unwrap();
        assert_eq!(maximum.load(AtomicOrdering::SeqCst), 3);
        let _ = fs::remove_dir_all(root);
    }
}
