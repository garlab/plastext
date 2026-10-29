//! Bounded, cancellable PDF work. Only the helper process calls the native PDF API.
//!
//! All public document operations block and must run off the UI thread. One broker
//! serializes work across all previews; its bounded queue contains no pixel data.

#[cfg(target_os = "macos")]
mod macos;
mod protocol;
mod worker;

pub use protocol::{PageInfo, SCALE_UNIT, TILE_SIZE, Tile, TilePixels};
pub use worker::worker_main;

use std::{
    fs::File,
    io::{Read, Seek, Write},
    path::Path,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

pub type Cancel = Arc<AtomicBool>;
pub const CANCELLED: &str = "PDF request cancelled";
pub const PREFETCH_INTERRUPTED: &str = "PDF prefetch yielded to visible work";
static FOREGROUND_JOBS: AtomicUsize = AtomicUsize::new(0);

struct ForegroundDemand(bool);
impl ForegroundDemand {
    fn new(prefetch: bool) -> Self {
        if !prefetch {
            FOREGROUND_JOBS.fetch_add(1, Ordering::SeqCst);
        }
        Self(!prefetch)
    }
}
impl Drop for ForegroundDemand {
    fn drop(&mut self) {
        if self.0 {
            FOREGROUND_JOBS.fetch_sub(1, Ordering::SeqCst);
        }
    }
}
fn foreground_waiting() -> bool {
    FOREGROUND_JOBS.load(Ordering::SeqCst) != 0
}
const SNAPSHOT_BUDGET: u64 = 2 * 1024 * 1024 * 1024;
static SNAPSHOT_BYTES: AtomicU64 = AtomicU64::new(0);
static NEXT_DOCUMENT: AtomicU64 = AtomicU64::new(1);

pub fn supported() -> bool {
    cfg!(target_os = "macos")
}

#[cfg(target_os = "macos")]
pub(crate) fn worker_resident_bytes(pid: u32) -> Option<u64> {
    macos::resident_bytes(pid)
}

pub fn is_pdf(path: &Path, head: &[u8]) -> bool {
    path.extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("pdf"))
        || head.starts_with(b"%PDF-")
}

pub(crate) fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err(CANCELLED.into())
    } else {
        Ok(())
    }
}

struct Reservation(u64);
impl Reservation {
    fn new(bytes: u64) -> Result<Self, String> {
        SNAPSHOT_BYTES
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.checked_add(bytes)
                    .filter(|&total| total <= SNAPSHOT_BUDGET)
            })
            .map_err(|_| {
                "PDF snapshots exceed the 2 GiB limit; close another preview or use Binary"
                    .to_string()
            })?;
        Ok(Self(bytes))
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        SNAPSHOT_BYTES.fetch_sub(self.0, Ordering::Relaxed);
    }
}

/// An immutable, private disk snapshot; no document-sized byte buffer is retained.
pub struct Document {
    pub id: u64,
    file: File,
    _reservation: Reservation,
}

impl Document {
    pub fn open(path: &Path, cancel: &Cancel) -> Result<(Arc<Self>, PageInfo), String> {
        if !supported() {
            return Err("PDF preview is currently available on macOS; use Binary".into());
        }
        let document = Arc::new(Self::snapshot(path, cancel)?);
        let info = document.page(1, cancel)?;
        Ok((document, info))
    }

    /// Capture bytes independently of parsing, so malformed PDFs remain readable
    /// in Binary mode. Keep this owner alive while using a cloned file descriptor.
    pub fn snapshot(path: &Path, cancel: &Cancel) -> Result<Self, String> {
        let copy = || -> Result<Self, Box<dyn std::error::Error>> {
            check_cancel(cancel)?;
            let mut source = File::open(path)?;
            let before = source.metadata()?;
            if !before.is_file() {
                return Err("PDF input must be a regular file".into());
            }
            let reservation = Reservation::new(before.len())?;
            // Unlinked immediately: a crash cannot leave gigabytes of snapshots
            // behind. The worker inherits this descriptor, never a source path.
            let mut output = tempfile::tempfile()?;
            let mut buffer = vec![0; 256 * 1024];
            let mut remaining = before.len();
            while remaining > 0 {
                check_cancel(cancel)?;
                let count = remaining.min(buffer.len() as u64) as usize;
                source.read_exact(&mut buffer[..count])?;
                output.write_all(&buffer[..count])?;
                remaining -= count as u64;
            }
            let after = source.metadata()?;
            if before.len() != after.len() || before.modified()? != after.modified()? {
                return Err("PDF changed while opening; retry Preview".into());
            }
            check_cancel(cancel)?;
            output.rewind()?;
            Ok(Self {
                id: NEXT_DOCUMENT.fetch_add(1, Ordering::Relaxed),
                file: output,
                _reservation: reservation,
            })
        };
        copy().map_err(|error| error.to_string())
    }

    pub fn page(self: &Arc<Self>, page: u32, cancel: &Cancel) -> Result<PageInfo, String> {
        self.page_with_priority(page, cancel, false)
    }

    /// Speculative metadata reads yield to any queued or running visible work.
    pub fn prefetch_page(self: &Arc<Self>, page: u32, cancel: &Cancel) -> Result<PageInfo, String> {
        self.page_with_priority(page, cancel, true)
    }

    fn page_with_priority(
        self: &Arc<Self>,
        page: u32,
        cancel: &Cancel,
        prefetch: bool,
    ) -> Result<PageInfo, String> {
        match submit(
            self.clone(),
            protocol::Operation::Info { page },
            cancel,
            prefetch,
        )? {
            Answer::Info(info) => Ok(info),
            _ => Err("Invalid PDF worker response".into()),
        }
    }

    /// Read access to exactly the bytes being previewed, for Binary mode.
    pub fn file(&self) -> std::io::Result<File> {
        self.file.try_clone()
    }

    pub fn tile(self: &Arc<Self>, tile: Tile, cancel: &Cancel) -> Result<TilePixels, String> {
        self.tile_with_priority(tile, cancel, false)
    }

    /// Speculative raster work is interruptible by visible work in any preview.
    pub fn prefetch_tile(
        self: &Arc<Self>,
        tile: Tile,
        cancel: &Cancel,
    ) -> Result<TilePixels, String> {
        self.tile_with_priority(tile, cancel, true)
    }

    fn tile_with_priority(
        self: &Arc<Self>,
        tile: Tile,
        cancel: &Cancel,
        prefetch: bool,
    ) -> Result<TilePixels, String> {
        match submit(
            self.clone(),
            protocol::Operation::Render(tile),
            cancel,
            prefetch,
        )? {
            Answer::Pixels(pixels) => Ok(pixels),
            _ => Err("Invalid PDF worker response".into()),
        }
    }
}

enum Answer {
    Info(PageInfo),
    Pixels(TilePixels),
}
struct Job {
    document: Arc<Document>,
    operation: protocol::Operation,
    cancel: Cancel,
    reply: mpsc::SyncSender<Result<Answer, String>>,
    prefetch: bool,
}

fn submit(
    document: Arc<Document>,
    operation: protocol::Operation,
    cancel: &Cancel,
    prefetch: bool,
) -> Result<Answer, String> {
    let _demand = ForegroundDemand::new(prefetch);
    if prefetch && foreground_waiting() {
        return Err(PREFETCH_INTERRUPTED.into());
    }
    static BROKER: OnceLock<mpsc::SyncSender<Job>> = OnceLock::new();
    let broker = BROKER.get_or_init(|| {
        let (send, receive) = mpsc::sync_channel::<Job>(16);
        std::thread::Builder::new()
            .name("pdf-broker".into())
            .spawn(move || {
                let mut worker: Option<worker::Worker> = None;
                loop {
                    // Retain native caches briefly while scrolling, then release the process.
                    let job = match receive.recv_timeout(Duration::from_secs(5)) {
                        Ok(job) => job,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            worker = None;
                            match receive.recv() {
                                Ok(job) => job,
                                Err(_) => break,
                            }
                        }
                        Err(_) => break,
                    };
                    if check_cancel(&job.cancel).is_err() {
                        continue;
                    }
                    if job.prefetch && foreground_waiting() {
                        let _ = job.reply.send(Err(PREFETCH_INTERRUPTED.into()));
                        continue;
                    }
                    let result = (|| {
                        if worker
                            .as_ref()
                            .is_none_or(|worker| worker.document_id() != job.document.id)
                        {
                            worker = None;
                            worker = Some(worker::Worker::spawn(job.document.clone())?);
                        }
                        worker
                            .as_mut()
                            .unwrap()
                            .run(job.operation, &job.cancel, job.prefetch)
                    })();
                    if result.is_err() {
                        worker = None;
                    }
                    let _ = job.reply.send(result);
                }
            })
            .expect("start PDF broker thread");
        send
    });
    let (reply, receive) = mpsc::sync_channel(1);
    let mut job = Job {
        document,
        operation,
        cancel: cancel.clone(),
        reply,
        prefetch,
    };
    loop {
        check_cancel(cancel)?;
        match broker.try_send(job) {
            Ok(()) => break,
            Err(mpsc::TrySendError::Full(returned)) => {
                job = returned;
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return Err("PDF worker service stopped".into()),
        }
    }
    loop {
        check_cancel(cancel)?;
        match receive.recv_timeout(Duration::from_millis(20)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return Err("PDF worker stopped; retry Preview".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn snapshot_is_immutable_and_cancellation_releases_disk_reservation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source.pdf");
        fs::write(&path, b"%PDF-original").unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut snapshot = Document::snapshot(&path, &cancel).unwrap();
        fs::write(&path, b"replacement").unwrap();
        let mut contents = Vec::new();
        snapshot.file.read_to_end(&mut contents).unwrap();
        assert_eq!(contents, b"%PDF-original");
        drop(snapshot);
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(Document::snapshot(&path, &cancel).err().unwrap(), CANCELLED);
    }

    #[test]
    fn identifies_pdf_before_text_or_binary_classification() {
        assert!(is_pdf(Path::new("paper.PDF"), b"broken"));
        assert!(is_pdf(Path::new("download"), b"%PDF-1.7"));
        assert!(!is_pdf(Path::new("paper.tex"), b"text"));
    }
}
