//! A single lazy diagram worker, shared by all panes. No UI-thread waits.
//! Protocol and I/O isolation adapted from mdviewer (MIT, Anthony GARCIA).
use plastext_assets::{
    Preview,
    mermaid::{self, MAX_PIXELS, MAX_SOURCE},
};
use std::{
    io::{self, Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
const BOUNDS: (u32, u32) = (8192, 8192);
const MAX_REQUESTS: usize = 32;
const TIMEOUT: Duration = Duration::from_secs(2);
type Answer = io::Result<Option<Preview>>;
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
                        if worker.as_ref().is_some_and(|w| w.completed >= MAX_REQUESTS) {
                            worker = None;
                        }
                        if worker.is_none() {
                            worker = Some(Worker::spawn(&std::env::current_exe()?)?);
                        }
                        worker.as_mut().unwrap().render(
                            &job.source,
                            BOUNDS,
                            job.light,
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

/// Private app entry point; invoke before initializing GPUI, settings or IPC.
pub fn worker_main() -> io::Result<()> {
    restrict_worker()?;
    serve(&mut io::stdin().lock(), &mut io::stdout().lock())
}

fn restrict_worker() -> io::Result<()> {
    #[cfg(unix)]
    unsafe {
        // A cumulative backstop if the parent dies mid-layout; workers recycle.
        libc::setrlimit(
            libc::RLIMIT_CPU,
            &libc::rlimit {
                rlim_cur: 8,
                rlim_max: 8,
            },
        );
        libc::setrlimit(
            libc::RLIMIT_CORE,
            &libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            },
        );
        libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
    #[cfg(target_os = "macos")]
    {
        use std::ffi::{CStr, c_char};
        unsafe extern "C" {
            fn sandbox_init(profile: *const c_char, flags: u64, error: *mut *mut c_char) -> i32;
            fn sandbox_free_error(error: *mut c_char);
        }
        // All fonts are embedded; this worker needs no document paths, external
        // images, system fonts, writes, network, or child processes. Fail closed.
        let profile = c"(version 1)(allow default)(deny network*)(deny file-write*)(deny file-read-data)(deny process-exec)(deny process-fork)";
        let mut error = std::ptr::null_mut();
        // SAFETY: static NUL-terminated profile and valid out-pointer.
        if unsafe { sandbox_init(profile.as_ptr(), 0, &mut error) } != 0 {
            let message = if error.is_null() {
                "Cannot restrict diagram worker".into()
            } else {
                unsafe { CStr::from_ptr(error) }
                    .to_string_lossy()
                    .into_owned()
            };
            if !error.is_null() {
                unsafe { sandbox_free_error(error) };
            }
            return Err(io::Error::other(message));
        }
    }
    Ok(())
}

fn serve(input: &mut impl Read, output: &mut impl Write) -> io::Result<()> {
    for _ in 0..MAX_REQUESTS {
        let mut header = [0; 17];
        match input.read(&mut header[..1])? {
            0 => return Ok(()),
            _ => input.read_exact(&mut header[1..])?,
        }
        let number = |slice: &[u8]| u32::from_le_bytes(slice.try_into().unwrap());
        let length = number(&header[12..16]) as usize;
        if &header[..4] != b"PMM1"
            || number(&header[4..8]) != BOUNDS.0
            || number(&header[8..12]) != BOUNDS.1
            || length == 0
            || length > MAX_SOURCE
            || header[16] > 1
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid diagram request",
            ));
        }
        let mut source = vec![0; length];
        input.read_exact(&mut source)?;
        let pixels = std::str::from_utf8(&source)
            .ok()
            .and_then(|s| mermaid::render(s, header[16] == 1));
        if let Some(p) = pixels {
            for n in [
                p.width,
                p.height,
                p.rgba.len() as u32,
                p.original_width,
                p.original_height,
            ] {
                output.write_all(&n.to_le_bytes())?;
            }
            output.write_all(&p.rgba)?;
        } else {
            output.write_all(&[0; 20])?;
        }
        output.flush()?;
    }
    Ok(())
}

fn response(input: &mut impl Read, bounds: (u32, u32)) -> io::Result<Option<Preview>> {
    let mut header = [0; 20];
    input.read_exact(&mut header)?;
    let number = |slice: &[u8]| u32::from_le_bytes(slice.try_into().unwrap());
    let (width, height, length) = (
        number(&header[..4]),
        number(&header[4..8]),
        number(&header[8..12]) as usize,
    );
    let original_width = number(&header[12..16]);
    let original_height = number(&header[16..20]);
    if width == 0 && height == 0 && length == 0 && original_width == 0 && original_height == 0 {
        return Ok(None);
    }
    let pixels = u64::from(width) * u64::from(height);
    if original_width == 0
        || original_height == 0
        || original_width > 8192
        || original_height > 8192
        || u64::from(original_width) * u64::from(original_height) > 4_000_000
        || width == 0
        || height == 0
        || width > bounds.0
        || height > bounds.1
        || pixels > MAX_PIXELS as u64
        || pixels * 4 != length as u64
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid Mermaid pixels",
        ));
    }
    let mut rgba = vec![0; length];
    input.read_exact(&mut rgba)?;
    Ok(Some(Preview {
        original_width,
        original_height,
        width,
        height,
        rgba,
    }))
}

type Request = (Vec<u8>, (u32, u32));
struct Worker {
    child: Child,
    requests: Option<mpsc::SyncSender<Request>>,
    replies: mpsc::Receiver<io::Result<Option<Preview>>>,
    thread: Option<JoinHandle<()>>,
    completed: usize,
}
impl Worker {
    fn spawn(program: &Path) -> io::Result<Self> {
        let child = Command::new(program)
            .arg("--mermaid-worker")
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        Self::connect(child)
    }

    fn connect(mut child: Child) -> io::Result<Self> {
        let mut input = child.stdin.take().unwrap();
        let mut output = child.stdout.take().unwrap();
        let (requests, incoming) = mpsc::sync_channel::<Request>(1);
        let (replies, outgoing) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("mermaid-io".into())
            .spawn(move || {
                while let Ok((message, bounds)) = incoming.recv() {
                    let result = input
                        .write_all(&message)
                        .and_then(|()| input.flush())
                        .and_then(|()| response(&mut output, bounds));
                    let failed = result.is_err();
                    if replies.send(result).is_err() || failed {
                        break;
                    }
                }
            });
        match thread {
            Ok(thread) => Ok(Self {
                child,
                requests: Some(requests),
                replies: outgoing,
                thread: Some(thread),
                completed: 0,
            }),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(error)
            }
        }
    }

    fn render(
        &mut self,
        source: &str,
        bounds: (u32, u32),
        light: bool,
        timeout: Duration,
        cancel: &AtomicBool,
    ) -> io::Result<Option<Preview>> {
        let mut message = Vec::with_capacity(17 + source.len());
        message.extend_from_slice(b"PMM1");
        message.extend_from_slice(&bounds.0.to_le_bytes());
        message.extend_from_slice(&bounds.1.to_le_bytes());
        message.extend_from_slice(&(source.len() as u32).to_le_bytes());
        message.push(u8::from(light));
        message.extend_from_slice(source.as_bytes());
        self.requests
            .as_ref()
            .unwrap()
            .try_send((message, bounds))
            .map_err(|_| io::Error::other("Mermaid worker unavailable"))?;
        let deadline = Instant::now() + timeout;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "Diagram cancelled",
                ));
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "Diagram timed out"));
            }
            #[cfg(target_os = "macos")]
            if crate::pdf::worker_resident_bytes(self.child.id())
                .is_some_and(|n| n > 256 * 1024 * 1024)
            {
                return Err(io::Error::other("Diagram exceeded its memory limit"));
            }
            match self.replies.recv_timeout(Duration::from_millis(20)) {
                Ok(result) => {
                    self.completed += 1;
                    return result;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Err(io::Error::other("Diagram worker stopped")),
            }
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        // The worker starts no descendants. Killing it closes both pipes and
        // unblocks the I/O thread even after a partial write or partial reply.
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.requests.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn worker_protocol_rejects_invalid_or_truncated_data_before_allocating() {
        let mut data = Vec::new();
        for n in [2u32, 1, 8, 1, 1] {
            data.extend_from_slice(&n.to_le_bytes());
        }
        data.extend_from_slice(&[255, 0, 0, 128, 0, 0, 255, 255]);
        let p = response(&mut &data[..], BOUNDS).unwrap().unwrap();
        assert_eq!((p.width, p.height), (2, 1));
        assert_eq!(p.rgba, data[20..]);
        assert!(response(&mut &data[..27], BOUNDS).is_err());
        assert!(response(&mut &data[..], (1, 1)).is_err());
        assert!(response(&mut &[0; 20][..], BOUNDS).unwrap().is_none());
        data[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(response(&mut &data[..], BOUNDS).is_err());
        let mut request = Vec::from(&b"PMM1"[..]);
        for n in [BOUNDS.0, BOUNDS.1, MAX_SOURCE as u32 + 1] {
            request.extend_from_slice(&n.to_le_bytes());
        }
        request.push(0);
        let mut output = Vec::new();
        assert!(serve(&mut &request[..], &mut output).is_err());
        assert!(output.is_empty());
    }
    #[cfg(unix)]
    #[test]
    fn timeouts_and_cancellation_kill_and_reap_partial_replies() {
        for cancel_after in [None, Some(Duration::from_millis(30))] {
            let child = Command::new("sh")
                .args(["-c", "printf '\\001'; exec sleep 30"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let mut worker = Worker::connect(child).unwrap();
            let pid = worker.child.id();
            let cancel = Arc::new(AtomicBool::new(false));
            let cancellation = cancel_after.map(|duration| {
                let cancel = cancel.clone();
                thread::spawn(move || {
                    thread::sleep(duration);
                    cancel.store(true, Ordering::Relaxed);
                })
            });
            let started = Instant::now();
            let error = worker
                .render(
                    "flowchart LR; A-->B",
                    BOUNDS,
                    false,
                    Duration::from_millis(100),
                    &cancel,
                )
                .err()
                .unwrap();
            assert_eq!(
                error.kind(),
                if cancel_after.is_some() {
                    io::ErrorKind::Interrupted
                } else {
                    io::ErrorKind::TimedOut
                }
            );
            drop(worker);
            if let Some(thread) = cancellation {
                thread.join().unwrap();
            }
            assert!(started.elapsed() < Duration::from_secs(3));
            // SAFETY: signal zero only checks whether the reaped child exists.
            assert_eq!(unsafe { libc::kill(pid as libc::pid_t, 0) }, -1);
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        }
    }
}
