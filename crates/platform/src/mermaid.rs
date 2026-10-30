//! App-wide queue and idle/recycling policy for shared native diagram workers.
pub use document_graphics::worker::worker_main;
use document_graphics::{
    Raster, RasterOptions, mermaid,
    worker::{MAX_REQUESTS, Worker},
};
use std::{
    io,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};
const TIMEOUT: Duration = Duration::from_secs(2);
type Answer = io::Result<Option<Raster>>;
struct Job {
    source: String,
    light: bool,
    cancel: Arc<AtomicBool>,
    reply: mpsc::SyncSender<Answer>,
}

/// Blocking: call from a background task. Rejected input never launches a worker.
pub fn render(source: &str, light: bool, cancel: &Arc<AtomicBool>) -> Answer {
    if !mermaid::admissible(source) {
        return Ok(None);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(io::ErrorKind::Interrupted.into());
    }
    static QUEUE: OnceLock<mpsc::SyncSender<Job>> = OnceLock::new();
    let queue = QUEUE.get_or_init(|| {
        let (send, recv) = mpsc::sync_channel::<Job>(8);
        thread::Builder::new()
            .name("mermaid-broker".into())
            .spawn(move || {
                let mut worker: Option<Worker> = None;
                loop {
                    let job = match recv.recv_timeout(Duration::from_secs(5)) {
                        Ok(job) => job,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            worker = None;
                            match recv.recv() {
                                Ok(job) => job,
                                Err(_) => break,
                            }
                        }
                        Err(_) => break,
                    };
                    if job.cancel.load(Ordering::Relaxed) {
                        continue;
                    }
                    let result = (|| {
                        if worker
                            .as_ref()
                            .is_some_and(|w| w.requests_completed() >= MAX_REQUESTS)
                        {
                            worker = None;
                        }
                        if worker.is_none() {
                            worker = Some(Worker::spawn(
                                &std::env::current_exe()?,
                                "--mermaid-worker",
                            )?);
                        }
                        worker.as_mut().unwrap().render(
                            &job.source,
                            job.light,
                            RasterOptions::scaled(2.),
                            TIMEOUT,
                            &job.cancel,
                        )
                    })();
                    if result.is_err() {
                        worker = None;
                    }
                    let _ = job.reply.send(result);
                }
            })
            .expect("start Mermaid broker");
        send
    });
    let (send, recv) = mpsc::sync_channel(1);
    queue
        .try_send(Job {
            source: source.into(),
            light,
            cancel: cancel.clone(),
            reply: send,
        })
        .map_err(|_| io::Error::new(io::ErrorKind::WouldBlock, "Diagram renderer busy"))?;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        match recv.recv_timeout(Duration::from_millis(20)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return Err(io::Error::other("Diagram worker stopped")),
        }
    }
}
