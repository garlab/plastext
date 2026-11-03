//! Image decoding lives in a disposable helper. All calls block: use background tasks.
use document_graphics::images::worker;
pub use document_graphics::images::{Frame, Info, Pixels, Region, SCALE_UNIT, TILE_EDGE};
#[derive(Clone)]
enum Operation {
    Info,
    Render(Vec<Region>),
}
use std::{
    fs::File,
    io::{Read, Seek, Write},
    path::Path,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};
pub use worker::worker_main;
pub type Cancel = Arc<AtomicBool>;
pub const CANCELLED: &str = "Image request cancelled";
const FILE_LIMIT: u64 = 256 * 1024 * 1024;
const SNAPSHOT_LIMIT: u64 = 1024 * 1024 * 1024;
static SNAPSHOT_BYTES: AtomicU64 = AtomicU64::new(0);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
fn check(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err(CANCELLED.into())
    } else {
        Ok(())
    }
}
struct Reservation(u64);
impl Drop for Reservation {
    fn drop(&mut self) {
        SNAPSHOT_BYTES.fetch_sub(self.0, Ordering::Relaxed);
    }
}
/// Immutable, unlinked disk snapshot shared by split panes. No full encoded buffer.
pub struct Document {
    pub id: u64,
    file: File,
    pub bytes: u64,
    _reservation: Reservation,
}
impl Document {
    pub fn open(path: &Path, cancel: &Cancel) -> Result<(Arc<Self>, Info), String> {
        let document = Arc::new(Self::snapshot(path, cancel)?);
        match submit(document.clone(), Operation::Info, cancel)? {
            Answer::Info(info) => Ok((document, info)),
            _ => Err("Unexpected image response".into()),
        }
    }
    fn snapshot(path: &Path, cancel: &Cancel) -> Result<Self, String> {
        let run = || -> Result<Self, Box<dyn std::error::Error>> {
            check(cancel)?;
            let mut source = File::open(path)?;
            let before = source.metadata()?;
            if !before.is_file() || before.len() > FILE_LIMIT {
                return Err("Image exceeds the 256 MiB file limit or is not a regular file".into());
            }
            SNAPSHOT_BYTES
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                    n.checked_add(before.len()).filter(|n| *n <= SNAPSHOT_LIMIT)
                })
                .map_err(|_| "Image snapshots exceed 1 GiB; close another image")?;
            let reservation = Reservation(before.len());
            let mut file = tempfile::tempfile()?;
            let mut buffer = vec![0; 256 * 1024];
            let mut remaining = before.len();
            while remaining > 0 {
                check(cancel)?;
                let n = remaining.min(buffer.len() as u64) as usize;
                source.read_exact(&mut buffer[..n])?;
                file.write_all(&buffer[..n])?;
                remaining -= n as u64;
            }
            let after = source.metadata()?;
            if before.len() != after.len() || before.modified()? != after.modified()? {
                return Err("Image changed while opening; retry Preview".into());
            }
            file.rewind()?;
            Ok(Self {
                id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                file,
                bytes: before.len(),
                _reservation: reservation,
            })
        };
        run().map_err(|e| e.to_string())
    }
    pub fn render(
        self: &Arc<Self>,
        info: &Info,
        regions: Vec<Region>,
        cancel: &Cancel,
    ) -> Result<Frame, String> {
        document_graphics::images::validate_regions(info, &regions)?;
        match submit(self.clone(), Operation::Render(regions), cancel)? {
            Answer::Frame(frame) => Ok(frame),
            _ => Err("Unexpected image response".into()),
        }
    }
}
enum Answer {
    Info(Info),
    Frame(Frame),
}
struct Job {
    document: Arc<Document>,
    operation: Operation,
    cancel: Cancel,
    reply: mpsc::SyncSender<Result<Answer, String>>,
}
fn submit(
    document: Arc<Document>,
    operation: Operation,
    cancel: &Cancel,
) -> Result<Answer, String> {
    check(cancel)?;
    static BROKER: OnceLock<mpsc::SyncSender<Job>> = OnceLock::new();
    let broker = BROKER.get_or_init(|| {
        let (send, receive) = mpsc::sync_channel::<Job>(8);
        std::thread::Builder::new()
            .name("image-broker".into())
            .spawn(move || {
                let mut worker: Option<(Arc<Document>, worker::Worker)> = None;
                loop {
                    let job = match receive.recv_timeout(Duration::from_secs(3)) {
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
                    if check(&job.cancel).is_err() {
                        continue;
                    }
                    let result = (|| {
                        if worker
                            .as_ref()
                            .is_none_or(|(document, _)| document.id != job.document.id)
                        {
                            worker = None;
                            worker = Some((
                                job.document.clone(),
                                worker::Worker::spawn(
                                    &std::env::current_exe().map_err(|e| e.to_string())?,
                                    "--image-worker",
                                    &job.document.file,
                                )?,
                            ));
                            if matches!(job.operation, Operation::Render(_)) {
                                worker.as_mut().unwrap().1.info(&job.cancel)?;
                            }
                        }
                        let worker = &mut worker.as_mut().unwrap().1;
                        match job.operation {
                            Operation::Info => worker.info(&job.cancel).map(Answer::Info),
                            Operation::Render(regions) => {
                                worker.render(regions, &job.cancel).map(Answer::Frame)
                            }
                        }
                    })();
                    if result.is_err() {
                        worker = None;
                    }
                    let _ = job.reply.send(result);
                }
            })
            .expect("start image broker");
        send
    });
    let (reply, receive) = mpsc::sync_channel(1);
    let mut job = Job {
        document,
        operation,
        cancel: cancel.clone(),
        reply,
    };
    loop {
        check(cancel)?;
        match broker.try_send(job) {
            Ok(()) => break,
            Err(mpsc::TrySendError::Full(returned)) => {
                job = returned;
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return Err("Image service stopped".into()),
        }
    }
    loop {
        check(cancel)?;
        match receive.recv_timeout(Duration::from_millis(20)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return Err("Image worker stopped".into()),
        }
    }
}
/// Static, bounded overview for Markdown; animation playback belongs to image tabs.
pub fn preview(path: &Path, cancel: &Cancel) -> Result<plastext_assets::Preview, String> {
    let (document, info) = Document::open(path, cancel)?;
    let mut frame = document.render(&info, vec![info.overview(0)], cancel)?;
    let pixels = frame.pixels.remove(0);
    Ok(plastext_assets::Preview {
        rgba: pixels.rgba,
        width: pixels.region.width,
        height: pixels.region.height,
        original_width: info.width,
        original_height: info.height,
    })
}

pub fn preview_data_uri(uri: &str, cancel: &Cancel) -> Result<plastext_assets::Preview, String> {
    check(cancel)?;
    let bytes = plastext_assets::data_uri_bytes(uri)?;
    let mut file = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    drop(bytes);
    preview(file.path(), cancel)
}
