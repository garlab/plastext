use super::{Answer, Cancel, Document, check_cancel, protocol::*};
#[cfg(target_os = "macos")]
use std::io::Write;
use std::{
    io::{self, Read},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(target_os = "macos")]
const WORKER_RSS_LIMIT: u64 = 512 * 1024 * 1024;

pub(super) struct Worker {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
    document: Arc<Document>,
}

impl Worker {
    pub fn document_id(&self) -> u64 {
        self.document.id
    }

    pub fn spawn(document: Arc<Document>) -> Result<Self, String> {
        if !super::supported() {
            return Err("PDF preview is unavailable on this platform".into());
        }
        let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
        command.arg("--pdf-worker");
        Self::start(command, document)
    }

    fn start(mut command: Command, document: Arc<Document>) -> Result<Self, String> {
        #[cfg(unix)]
        {
            use std::os::{fd::AsRawFd, unix::process::CommandExt};
            let source = document.file.as_raw_fd();
            // SAFETY: pre_exec performs only async-signal-safe descriptor syscalls;
            // document keeps the source open until spawn has completed.
            unsafe {
                command.pre_exec(move || {
                    if libc::dup2(source, 3) < 0 || libc::fcntl(3, libc::F_SETFD, 0) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Cannot start PDF worker: {e}"))?;
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let worker = Self {
            child,
            input,
            output,
            document,
        };
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: the pipe descriptors are owned and valid for this call.
            let result = unsafe {
                let fd = worker.output.as_raw_fd();
                let flags = libc::fcntl(fd, libc::F_GETFL);
                if flags < 0 {
                    -1
                } else {
                    libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK)
                }
            };
            if result < 0 {
                return Err(format!(
                    "Cannot configure PDF pipe: {}",
                    io::Error::last_os_error()
                ));
            }
        }
        Ok(worker)
    }

    pub fn run(
        &mut self,
        operation: Operation,
        cancel: &Cancel,
        prefetch: bool,
    ) -> Result<Answer, String> {
        check_cancel(cancel)?;
        // Only one short request is ever outstanding, so the empty pipe has room
        // for this bounded header. Pixel replies are read in cancellable chunks.
        write_message(&mut self.input, &Request { operation })
            .map_err(|e| format!("PDF worker stopped: {e}"))?;
        let mut reader = Interruptible {
            worker: self,
            cancel,
            deadline: Instant::now() + REQUEST_TIMEOUT,
            memory_check: Instant::now(),
            prefetch,
            #[cfg(target_os = "macos")]
            memory_limit: WORKER_RSS_LIMIT,
        };
        let response: Response = read_message(&mut reader).map_err(|e| e.to_string())?;
        match response {
            Response::Info(info) => {
                reader.check_resources(true).map_err(|e| e.to_string())?;
                Ok(Answer::Info(info.validate()?))
            }
            Response::Error(error) => Err(error),
            Response::Pixels { width, height } => {
                let Operation::Render(tile) = operation else {
                    return Err("Unexpected PDF pixels".into());
                };
                if width == 0 || height == 0 || width > TILE_SIZE || height > TILE_SIZE {
                    return Err("PDF worker returned an oversized tile".into());
                }
                let mut bgra = vec![0; width as usize * height as usize * 4];
                reader.read_exact(&mut bgra).map_err(|e| e.to_string())?;
                reader.check_resources(true).map_err(|e| e.to_string())?;
                Ok(Answer::Pixels(TilePixels {
                    tile,
                    width,
                    height,
                    bgra,
                }))
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Never leave a native render running after cancellation or a timeout.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Interruptible<'a> {
    worker: &'a mut Worker,
    cancel: &'a Cancel,
    deadline: Instant,
    memory_check: Instant,
    prefetch: bool,
    #[cfg(target_os = "macos")]
    memory_limit: u64,
}
impl Interruptible<'_> {
    fn check_resources(&mut self, force: bool) -> io::Result<()> {
        check_cancel(self.cancel).map_err(io::Error::other)?;
        if self.prefetch && super::foreground_waiting() {
            return Err(io::Error::other(super::PREFETCH_INTERRUPTED));
        }
        let now = Instant::now();
        if now >= self.deadline {
            return Err(io::Error::other(
                "PDF page exceeded the 10 second work limit; use Binary or retry",
            ));
        }
        if force || now >= self.memory_check {
            self.memory_check = now + Duration::from_millis(100);
            #[cfg(target_os = "macos")]
            {
                let rss =
                    super::macos::resident_bytes(self.worker.child.id()).ok_or_else(|| {
                        io::Error::other("Cannot monitor PDF worker memory; retry Preview")
                    })?;
                if rss > self.memory_limit {
                    return Err(io::Error::other(
                        "PDF worker exceeded 512 MiB; use Binary or retry",
                    ));
                }
            }
        }
        Ok(())
    }
}
impl Read for Interruptible<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            self.check_resources(false)?;
            match self.worker.output.read(bytes) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    #[cfg(unix)]
                    {
                        use std::os::fd::AsRawFd;
                        let mut fd = libc::pollfd {
                            fd: self.worker.output.as_raw_fd(),
                            events: libc::POLLIN,
                            revents: 0,
                        };
                        // SAFETY: one live descriptor. Wake immediately for bytes,
                        // or within 10 ms to check cancellation and resource limits.
                        if unsafe { libc::poll(&mut fd, 1, 10) } < 0 {
                            let error = io::Error::last_os_error();
                            if error.kind() != io::ErrorKind::Interrupted {
                                return Err(error);
                            }
                        }
                    }
                    #[cfg(not(unix))]
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                result => return result,
            }
        }
    }
}

/// Private app entry point. Must be called before initializing any GUI runtime.
pub fn worker_main() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        if let Err(error) = super::macos::restrict_worker() {
            write_message(
                &mut io::stdout().lock(),
                &Response::Error(format!("Cannot isolate PDF renderer: {error}")),
            )
            .map_err(|e| e.to_string())?;
            return Ok(());
        }
        let mut input = io::stdin().lock();
        let mut output = io::stdout().lock();
        // The broker passes only an immutable snapshot as descriptor 3. There is
        // no path in the protocol and no request that can open another document.
        // Also bound work if the parent crashes before it can enforce its deadline.
        // SAFETY: process-local alarm, using the default terminating disposition.
        unsafe {
            libc::alarm(12);
        }
        let document = super::macos::NativeDocument::open(std::path::Path::new("/dev/fd/3"));
        unsafe {
            libc::alarm(0);
        }
        loop {
            let request: Request = match read_message(&mut input) {
                Ok(request) => request,
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
                Err(e) => return Err(e.to_string()),
            };
            let result = (|| -> Result<Answer, String> {
                unsafe {
                    libc::alarm(12);
                }
                let native = document.as_ref().map_err(Clone::clone)?;
                match request.operation {
                    Operation::Info { page } => native.info(page).map(Answer::Info),
                    Operation::Render(tile) => native.render(tile).map(Answer::Pixels),
                }
            })();
            unsafe {
                libc::alarm(0);
            }
            match result {
                Ok(Answer::Info(info)) => write_message(&mut output, &Response::Info(info)),
                Ok(Answer::Pixels(pixels)) => write_message(
                    &mut output,
                    &Response::Pixels {
                        width: pixels.width,
                        height: pixels.height,
                    },
                )
                .and_then(|_| output.write_all(&pixels.bgra))
                .and_then(|_| output.flush()),
                Err(error) => write_message(&mut output, &Response::Error(error)),
            }
            .map_err(|e| e.to_string())?;
        }
    }
    #[cfg(not(target_os = "macos"))]
    Err("PDF preview is currently available on macOS".into())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn cancelling_an_unresponsive_worker_kills_and_reaps_it() {
        let source = tempfile::NamedTempFile::new().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let document = Arc::new(Document::snapshot(source.path(), &cancel).unwrap());
        let mut command = Command::new("/bin/sleep");
        command.arg("60");
        let mut worker = Worker::start(command, document).unwrap();
        let pid = worker.child.id();
        let flag = cancel.clone();
        let trigger = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            flag.store(true, Ordering::Relaxed);
        });
        let start = Instant::now();
        let error = worker
            .run(Operation::Info { page: 1 }, &cancel, false)
            .err()
            .unwrap();
        assert_eq!(error, super::super::CANCELLED);
        drop(worker);
        trigger.join().unwrap();
        assert!(start.elapsed() < Duration::from_secs(1));
        // SAFETY: signal zero only queries existence. The exact child was reaped.
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
    }

    #[test]
    fn visible_work_interrupts_an_unresponsive_prefetch_and_reaps_it() {
        let source = tempfile::NamedTempFile::new().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let document = Arc::new(Document::snapshot(source.path(), &cancel).unwrap());
        let mut command = Command::new("/bin/sleep");
        command.arg("60");
        let mut worker = Worker::start(command, document).unwrap();
        let pid = worker.child.id();
        let (done, receive) = std::sync::mpsc::channel();
        let trigger = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            // Demand remains outstanding until the speculative call returns.
            let _demand = super::super::ForegroundDemand::new(false);
            let _ = receive.recv_timeout(Duration::from_secs(2));
        });
        let start = Instant::now();
        let error = worker
            .run(Operation::Info { page: 1 }, &cancel, true)
            .err()
            .unwrap();
        drop(worker);
        let _ = done.send(());
        trigger.join().unwrap();
        assert_eq!(error, super::super::PREFETCH_INTERRUPTED);
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(
            !cancel.load(Ordering::Relaxed),
            "visible work, not cancellation, caused the yield"
        );
        // SAFETY: signal zero only queries existence. The exact child was reaped.
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn resident_memory_and_deadline_guards_stop_unresponsive_work() {
        let source = tempfile::NamedTempFile::new().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let document = Arc::new(Document::snapshot(source.path(), &cancel).unwrap());
        let mut command = Command::new("/bin/sleep");
        command.arg("60");
        let mut worker = Worker::start(command, document).unwrap();
        let mut reader = Interruptible {
            worker: &mut worker,
            cancel: &cancel,
            deadline: Instant::now() + Duration::from_secs(1),
            memory_check: Instant::now(),
            memory_limit: WORKER_RSS_LIMIT,
            prefetch: false,
        };
        reader.check_resources(true).unwrap();
        // Exercise the real OS RSS query without allocating hundreds of MB in a test.
        reader.memory_limit = 1;
        assert!(
            reader
                .check_resources(true)
                .unwrap_err()
                .to_string()
                .contains("512 MiB")
        );
        reader.deadline = Instant::now();
        assert!(
            reader
                .check_resources(true)
                .unwrap_err()
                .to_string()
                .contains("10 second")
        );
    }
}
