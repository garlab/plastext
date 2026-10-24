//! Single-instance hand-off (spec CLI-1): later launches pass their arguments
//! to the running app over a local socket instead of starting another one.

use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};

/// A launch's arguments, to be resolved against the directory it ran in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenRequest {
    pub cwd: PathBuf,
    pub args: Vec<OsString>,
}

impl OpenRequest {
    /// The current process's working directory and arguments.
    pub fn from_env() -> io::Result<Self> {
        Ok(Self {
            cwd: std::env::current_dir()?,
            args: std::env::args_os().skip(1).collect(),
        })
    }

    /// Resolves `path` from the request against its working directory.
    pub fn resolve(&self, path: &Path) -> PathBuf {
        let joined = self.cwd.join(path);
        std::path::absolute(&joined).unwrap_or(joined)
    }
}

pub use imp::{Server, listen, send};

#[cfg(unix)]
mod imp {
    use std::{
        ffi::OsString,
        fs,
        io::{self, Read as _, Write as _},
        os::unix::{
            ffi::{OsStrExt as _, OsStringExt as _},
            fs::DirBuilderExt as _,
            net::{UnixListener, UnixStream},
        },
        path::PathBuf,
        thread,
        time::Duration,
    };

    use super::OpenRequest;

    const TIMEOUT: Duration = Duration::from_secs(2);
    /// Requests are a few paths; anything bigger isn't from our launcher.
    const MAX_REQUEST: u64 = 1024 * 1024;

    /// The listening socket; requests arrive on a background thread.
    pub struct Server {
        _private: (),
    }

    /// Sends `request` to the running instance. Fails if there's none.
    pub fn send(request: &OpenRequest) -> io::Result<()> {
        let mut stream = UnixStream::connect(socket_path()?)?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        stream.write_all(&encode(request))?;
        stream.shutdown(std::net::Shutdown::Write)
    }

    /// Listens for requests from later launches and passes each to `handle`
    /// on a background thread. Fails with `AddrInUse` if another instance is
    /// listening; a socket left behind by a crashed instance is replaced.
    pub fn listen(handle: impl Fn(OpenRequest) + Send + 'static) -> io::Result<Server> {
        let path = socket_path()?;
        let listener = match UnixListener::bind(&path) {
            Ok(listener) => listener,
            Err(err) if err.kind() == io::ErrorKind::AddrInUse => {
                if UnixStream::connect(&path).is_ok() {
                    return Err(err);
                }
                fs::remove_file(&path)?;
                UnixListener::bind(&path)?
            }
            Err(err) => return Err(err),
        };
        thread::Builder::new()
            .name("plastext-instance".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    // A failed or malformed request only loses that request.
                    if let Some(request) = stream
                        .ok()
                        .and_then(|stream| receive(stream).ok())
                        .flatten()
                    {
                        handle(request);
                    }
                }
            })?;
        Ok(Server { _private: () })
    }

    fn receive(stream: UnixStream) -> io::Result<Option<OpenRequest>> {
        // The timeout only guards against a client that stays connected. macOS
        // refuses to set it (EINVAL) once the client has hung up, and then the
        // request is fully buffered and reading can't block anyway.
        stream.set_read_timeout(Some(TIMEOUT)).ok();
        let mut bytes = Vec::new();
        stream.take(MAX_REQUEST).read_to_end(&mut bytes)?;
        Ok(decode(&bytes))
    }

    /// The socket lives in a per-user directory: `$TMPDIR` on macOS,
    /// `$XDG_RUNTIME_DIR` on Linux, or a private directory under `/tmp`.
    fn socket_path() -> io::Result<PathBuf> {
        if let Some(path) = std::env::var_os("PLASTEXT_SOCKET") {
            return Ok(path.into());
        }
        let dir = if cfg!(target_os = "macos") {
            std::env::temp_dir()
        } else if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
            dir.into()
        } else {
            let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
            let dir = std::env::temp_dir().join(format!("plastext-{user}"));
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&dir)?;
            dir
        };
        Ok(dir.join("plastext.sock"))
    }

    /// The working directory, then each argument, each followed by a NUL byte,
    /// which can't occur in paths or arguments.
    pub(super) fn encode(request: &OpenRequest) -> Vec<u8> {
        let mut bytes = Vec::new();
        for part in std::iter::once(request.cwd.as_os_str())
            .chain(request.args.iter().map(|arg| arg.as_os_str()))
        {
            bytes.extend_from_slice(part.as_bytes());
            bytes.push(0);
        }
        bytes
    }

    pub(super) fn decode(bytes: &[u8]) -> Option<OpenRequest> {
        let mut parts = bytes.strip_suffix(&[0])?.split(|&byte| byte == 0);
        let cwd = PathBuf::from(OsString::from_vec(parts.next()?.to_vec()));
        if !cwd.is_absolute() {
            return None;
        }
        let args = parts
            .map(|part| OsString::from_vec(part.to_vec()))
            .collect();
        Some(OpenRequest { cwd, args })
    }
}

#[cfg(not(unix))]
mod imp {
    use std::io;

    use super::OpenRequest;

    /// Single-instance hand-off isn't implemented here yet, so every launch
    /// starts its own app.
    pub struct Server {
        _private: (),
    }

    pub fn send(_: &OpenRequest) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }

    pub fn listen(_: impl Fn(OpenRequest) + Send + 'static) -> io::Result<Server> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn resolves_paths_against_the_request_directory() {
        let request = OpenRequest {
            cwd: PathBuf::from("/work/project"),
            args: Vec::new(),
        };
        assert_eq!(request.resolve(Path::new(".")), Path::new("/work/project"));
        assert_eq!(
            request.resolve(Path::new("./src/lib.rs")),
            Path::new("/work/project/src/lib.rs")
        );
        assert_eq!(
            request.resolve(Path::new("/etc/hosts")),
            Path::new("/etc/hosts")
        );
    }

    #[cfg(unix)]
    #[test]
    fn requests_round_trip() {
        let request = OpenRequest {
            cwd: PathBuf::from("/work/project"),
            args: vec![
                "src/lib.rs:12".into(),
                "a file with spaces".into(),
                "".into(),
            ],
        };
        assert_eq!(imp::decode(&imp::encode(&request)), Some(request));
        assert_eq!(imp::decode(b"relative\0"), None);
        assert_eq!(imp::decode(b"/no-terminator"), None);
    }

    #[cfg(unix)]
    #[test]
    fn hands_requests_to_the_listening_instance() {
        use std::sync::mpsc;

        let dir = tempfile::tempdir().unwrap();
        // SAFETY: this is the only test that touches the environment.
        unsafe { std::env::set_var("PLASTEXT_SOCKET", dir.path().join("test.sock")) };
        let request = OpenRequest {
            cwd: PathBuf::from("/work"),
            args: vec!["notes.md".into()],
        };
        assert!(send(&request).is_err(), "nothing is listening yet");

        let (sender, receiver) = mpsc::channel();
        let _server = listen(move |request| sender.send(request).unwrap()).unwrap();
        let second = listen(|_| {});
        assert_eq!(
            second.err().map(|err| err.kind()),
            Some(io::ErrorKind::AddrInUse)
        );

        send(&request).unwrap();
        let received = receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(received, request);
    }
}
