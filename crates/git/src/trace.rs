//! Opt-in Git diagnostics. A bounded channel keeps file I/O off UI/watch threads.
use std::{
    fmt::{self, Write as _},
    io::Write as _,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const MAX_LINE: usize = 4096;
const MAX_BYTES: usize = 8 * 1024 * 1024;
struct Trace {
    send: mpsc::SyncSender<String>,
    dropped: Arc<AtomicU64>,
    stopped: Arc<AtomicBool>,
    started: Instant,
    all_events: bool,
}
static TRACE: OnceLock<Option<Trace>> = OnceLock::new();

/// Set PLASTEXT_GIT_TRACE=1 before launching. Each process gets a private log
/// in /tmp (the system temp directory on Windows), capped at 8 MiB.
pub fn enabled() -> bool {
    TRACE
        .get_or_init(|| match std::env::var("PLASTEXT_GIT_TRACE").as_deref() {
            Ok("1") => start(false),
            Ok("all") => start(true),
            _ => None,
        })
        .as_ref()
        .is_some_and(|trace| !trace.stopped.load(Ordering::Relaxed))
}

pub(crate) fn all_events() -> bool {
    TRACE
        .get()
        .and_then(Option::as_ref)
        .is_some_and(|trace| trace.all_events)
}

fn start(all_events: bool) -> Option<Trace> {
    let (send, incoming) = mpsc::sync_channel::<String>(1024);
    let dropped = Arc::new(AtomicU64::new(0));
    let stopped = Arc::new(AtomicBool::new(false));
    let lost = dropped.clone();
    let done = stopped.clone();
    std::thread::Builder::new()
        .name("git-trace".into())
        .spawn(move || {
            let result = (|| -> std::io::Result<()> {
                #[cfg(unix)]
                let directory = std::path::PathBuf::from("/tmp");
                #[cfg(not(unix))]
                let directory = std::env::temp_dir();
                let epoch = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default();
                let path = directory.join(format!(
                    "plastext-git-{}-{}.log",
                    std::process::id(),
                    epoch.as_millis()
                ));
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = options.open(&path)?;
                eprintln!("Git trace: {}", path.display());
                writeln!(
                    file,
                    "git_trace pid={} unix_ms={} limit_bytes={MAX_BYTES}",
                    std::process::id(),
                    epoch.as_millis()
                )?;
                let mut bytes = 128;
                for line in incoming {
                    let skipped = lost.swap(0, Ordering::Relaxed);
                    if skipped != 0 {
                        writeln!(file, "trace_dropped count={skipped}")?;
                        bytes += 64;
                    }
                    if bytes + line.len() + 128 >= MAX_BYTES {
                        writeln!(file, "trace_stopped reason=size_limit")?;
                        break;
                    }
                    file.write_all(line.as_bytes())?;
                    bytes += line.len();
                }
                Ok(())
            })();
            done.store(true, Ordering::Relaxed);
            if let Err(error) = result {
                eprintln!("Git trace stopped: {error}");
            }
        })
        .ok()?;
    Some(Trace {
        send,
        dropped,
        stopped,
        started: Instant::now(),
        all_events,
    })
}

struct Line(String);
impl fmt::Write for Line {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let mut end = text.len().min(MAX_LINE.saturating_sub(self.0.len()));
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        // Keep one record per line even for unusual filenames.
        self.0.extend(
            text[..end]
                .chars()
                .map(|c| if matches!(c, '\n' | '\r') { ' ' } else { c }),
        );
        Ok(())
    }
}

#[doc(hidden)]
pub fn record(args: fmt::Arguments<'_>) {
    let Some(Some(trace)) = TRACE.get() else {
        return;
    };
    let mut line = Line(String::new());
    let _ = write!(line, "{}ms {args}", trace.started.elapsed().as_millis());
    line.0.push('\n');
    if matches!(
        trace.send.try_send(line.0),
        Err(mpsc::TrySendError::Full(_))
    ) {
        trace.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

/// Arguments aren't evaluated when tracing is disabled.
#[macro_export]
macro_rules! git_trace {
    ($($args:tt)*) => {
        if $crate::trace::enabled() {
            $crate::trace::record(format_args!($($args)*));
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trace_lines_are_bounded_and_keep_unicode_boundaries() {
        let mut line = Line(String::new());
        write!(line, "{}\nextra", "é".repeat(MAX_LINE)).unwrap();
        assert!(line.0.len() <= MAX_LINE);
        assert!(!line.0.contains('\n'));
        assert!(line.0.ends_with('é'));
    }
}
