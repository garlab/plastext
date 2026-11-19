//! Render a complete picture from an unsaved LaTeX snapshot in a disposable native worker.
use document_graphics::{MAX_EDGE, MAX_PIXELS, Raster};
use std::{
    io::{self, Read, Write},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

// Includes the reader's scoped macro definitions as well as the picture itself.
const MAX_INPUT: usize = 512 * 1024;
const MAX_ERROR: usize = 1024;

pub fn render(source: &str, cancel: &AtomicBool) -> io::Result<Raster> {
    if !cfg!(target_os = "macos") {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Isolated TikZ preview requires macOS",
        ));
    }
    if source.len() > MAX_INPUT {
        return Err(io::Error::other("TikZ input exceeds the preview limit"));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(io::ErrorKind::Interrupted.into());
    }
    let mut child = Command::new(std::env::current_exe()?)
        .arg("--tikz-worker")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut input = child.stdin.take().unwrap();
    let output = child.stdout.take().unwrap();
    thread::scope(|scope| {
        let (reply, receive) = mpsc::channel();
        scope.spawn(move || {
            let written = input.write_all(source.as_bytes());
            drop(input);
            let result = written.and_then(|()| read_raster(output));
            let _ = reply.send(result);
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let answer = loop {
            if cancel.load(Ordering::Relaxed) {
                break Err(io::ErrorKind::Interrupted.into());
            }
            if Instant::now() >= deadline {
                break Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "TikZ preview timed out",
                ));
            }
            #[cfg(target_os = "macos")]
            if crate::worker_limits::resident_bytes(child.id())
                .is_some_and(|rss| rss > 256 * 1024 * 1024)
            {
                break Err(io::Error::other("TikZ preview exceeded its memory limit"));
            }
            match receive.recv_timeout(Duration::from_millis(10)) {
                Ok(result) => break result,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => break Err(io::Error::other("TikZ renderer stopped")),
            }
        };
        // Close both pipes even when cancellation interrupts a blocked read or write.
        let _ = child.kill();
        let _ = child.wait();
        answer
    })
}

fn read_raster(mut output: impl Read) -> io::Result<Raster> {
    let mut number = || {
        let mut bytes = [0; 4];
        output.read_exact(&mut bytes)?;
        Ok::<_, io::Error>(u32::from_le_bytes(bytes))
    };
    let width = number()?;
    if width == 0 {
        let mut message = String::new();
        output.take(MAX_ERROR as u64).read_to_string(&mut message)?;
        return Err(io::Error::other(message));
    }
    let height = number()?;
    let original_width = number()?;
    let original_height = number()?;
    if [width, height, original_width, original_height]
        .iter()
        .any(|n| !(1..=MAX_EDGE).contains(n))
        || u64::from(width) * u64::from(height) > MAX_PIXELS as u64
    {
        return Err(io::Error::other("Invalid TikZ preview dimensions"));
    }
    let mut rgba = vec![0; width as usize * height as usize * 4];
    output.read_exact(&mut rgba)?;
    Ok(Raster {
        width,
        height,
        original_width,
        original_height,
        rgba,
    })
}

/// One request per process. The only input is UTF-8 on stdin, terminated by EOF.
#[cfg(target_os = "macos")]
pub fn worker_main() -> io::Result<()> {
    crate::worker_limits::restrict_worker(c"(version 1)(allow default)(deny network*)(deny file-write*)(deny file-read-data)(deny process-exec)(deny process-fork)")
        .map_err(io::Error::other)?;
    // SAFETY: backstop for this dedicated child if its parent disappears.
    unsafe {
        libc::alarm(8);
    }

    let mut source = String::new();
    io::stdin()
        .take(MAX_INPUT as u64 + 1)
        .read_to_string(&mut source)?;
    let mut output = io::stdout().lock();
    match rasterize(&source) {
        Ok(image) => {
            for n in [
                image.width,
                image.height,
                image.original_width,
                image.original_height,
            ] {
                output.write_all(&n.to_le_bytes())?;
            }
            output.write_all(&image.rgba)
        }
        Err(error) => {
            output.write_all(&0u32.to_le_bytes())?;
            output.write_all(
                &error.as_bytes()[..error.floor_char_boundary(error.len().min(MAX_ERROR))],
            )
        }
    }
}

#[cfg(target_os = "macos")]
fn rasterize(source: &str) -> Result<Raster, String> {
    use resvg::{tiny_skia, usvg};
    if source.len() > MAX_INPUT {
        return Err("TikZ input exceeds the preview limit".into());
    }
    let figure = tikra::convert(source).map_err(|e| e.to_string())?;
    if let Some(warning) = figure.warnings.first() {
        return Err(format!("TikZ: {warning}"));
    }
    let options = usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_string: Box::new(|_, _| None),
            ..Default::default()
        },
        ..Default::default()
    };
    let tree = usvg::Tree::from_str(&figure.svg, &options).map_err(|e| e.to_string())?;
    let logical = tree.size().to_int_size();
    let (w, h) = (logical.width(), logical.height());
    if w > MAX_EDGE || h > MAX_EDGE {
        return Err("TikZ picture exceeds the preview dimensions".into());
    }
    let scale = 2_f64
        .min((MAX_PIXELS as f64 / (f64::from(w) * f64::from(h))).sqrt())
        .min(f64::from(MAX_EDGE) / f64::from(w.max(h)));
    let width = ((f64::from(w) * scale) as u32).max(1);
    let height = ((f64::from(h) * scale) as u32).max(1);
    let mut pixels = tiny_skia::Pixmap::new(width, height).ok_or("Invalid TikZ preview size")?;
    // TikZ colors are specified for paper, including black and gray math labels.
    pixels.fill(tiny_skia::Color::WHITE);
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(
            width as f32 / tree.size().width(),
            height as f32 / tree.size().height(),
        ),
        &mut pixels.as_mut(),
    );
    Ok(Raster {
        width,
        height,
        original_width: w,
        original_height: h,
        rgba: pixels.take(),
    })
}
