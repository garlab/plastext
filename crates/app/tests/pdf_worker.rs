//! Exercise the real app's headless worker, including PDF rotation, clipping and
//! framing. Fixtures are generated locally; no PDFs or raw pixel logs enter git.
#![cfg(target_os = "macos")]

use serde_json::{Value, json};
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

struct Helper {
    child: Arc<Mutex<Child>>,
    input: ChildStdin,
    output: ChildStdout,
    done: mpsc::Sender<()>,
}
impl Helper {
    fn new(path: &Path) -> Self {
        let file = File::open(path).unwrap();
        let fd = file.as_raw_fd();
        let mut command = Command::new(env!("CARGO_BIN_EXE_plastext-app"));
        command
            .arg("--pdf-worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        // SAFETY: only async-signal-safe syscalls; file remains alive through spawn.
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(fd, 3) < 0 || libc::fcntl(3, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().unwrap();
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let child = Arc::new(Mutex::new(child));
        let watchdog = child.clone();
        let (done, receive) = mpsc::channel();
        std::thread::spawn(move || {
            if receive.recv_timeout(Duration::from_secs(20)).is_err() {
                let _ = watchdog.lock().unwrap().kill();
            }
        });
        Self {
            child,
            input,
            output,
            done,
        }
    }
    fn request(&mut self, operation: Value) -> (Value, Vec<u8>) {
        let message = serde_json::to_vec(&json!({ "operation": operation })).unwrap();
        self.input
            .write_all(&(message.len() as u32).to_le_bytes())
            .unwrap();
        self.input.write_all(&message).unwrap();
        self.input.flush().unwrap();
        let mut size = [0; 4];
        self.output
            .read_exact(&mut size)
            .expect("worker reply (native sandbox tests require an unsandboxed test runner)");
        let size = u32::from_le_bytes(size) as usize;
        assert!(size <= 16 * 1024);
        let mut message = vec![0; size];
        self.output.read_exact(&mut message).unwrap();
        let response: Value = serde_json::from_slice(&message).unwrap();
        let mut pixels = Vec::new();
        if let Some(tile) = response.get("Pixels") {
            let width = tile["width"].as_u64().unwrap();
            let height = tile["height"].as_u64().unwrap();
            assert!(width <= 512 && height <= 512);
            pixels.resize((width * height * 4) as usize, 0);
            self.output.read_exact(&mut pixels).unwrap();
        }
        (response, pixels)
    }
}
impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.done.send(());
        let mut child = self.child.lock().unwrap();
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn fixture(path: &Path, pages: usize, rotated: bool) {
    let content = b"1 0 0 rg 0 400 600 400 re f 0 0 1 rg 0 0 600 400 re f";
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!(
            "<< /Type /Pages /Count {pages} /Kids [{}] >>",
            (0..pages)
                .map(|i| format!("{} 0 R", i + 4))
                .collect::<Vec<_>>()
                .join(" ")
        )
        .into_bytes(),
        [
            format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
            content,
            b"\nendstream",
        ]
        .concat(),
    ];
    for _ in 0..pages {
        objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /CropBox [0 0 600 800] /Rotate {} /Resources << >> /Contents 3 0 R >>", if rotated {90} else {0}).into_bytes());
    }
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        writeln!(pdf, "{} 0 obj", index + 1).unwrap();
        pdf.extend(object);
        pdf.extend(b"\nendobj\n");
    }
    let xref = pdf.len();
    write!(pdf, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).unwrap();
    for offset in &offsets[1..] {
        writeln!(pdf, "{offset:010} 00000 n ").unwrap();
    }
    write!(
        pdf,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    )
    .unwrap();
    fs::write(path, pdf).unwrap();
}

#[test]
fn native_worker_renders_top_down_bgra_tiles_and_rotated_pages() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("colors.pdf");
    fixture(&path, 2, false);
    let mut helper = Helper::new(&path);
    let (info, _) = helper.request(json!({"Info": {"page": 1}}));
    assert_eq!(info["Info"]["pages"], 2);
    let (reply, pixels) =
        helper.request(json!({"Render": {"page": 1, "scale": 4096, "x": 0, "y": 0}}));
    assert_eq!(reply["Pixels"]["width"], 512);
    assert_eq!(&pixels[0..4], &[0, 0, 255, 255], "top of page is red");
    assert_eq!(
        &pixels[450 * 512 * 4..450 * 512 * 4 + 4],
        &[255, 0, 0, 255],
        "lower page is blue"
    );
    let (_, enlarged) =
        helper.request(json!({"Render": {"page": 1, "scale": 8192, "x": 0, "y": 0}}));
    assert!(
        enlarged
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| *pixel == [0, 0, 255, 255]),
        "Retina zoom must enlarge content instead of centering it unscaled"
    );
    let (reply, pixels) =
        helper.request(json!({"Render": {"page": 2, "scale": 4096, "x": 512, "y": 512}}));
    assert_eq!(reply["Pixels"], json!({"width": 88, "height": 288}));
    assert!(
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| *pixel == [255, 0, 0, 255])
    );

    let rotated = dir.path().join("rotated.pdf");
    fixture(&rotated, 1, true);
    let mut helper = Helper::new(&rotated);
    let (info, _) = helper.request(json!({"Info": {"page": 1}}));
    assert_eq!(info["Info"]["width"], 800.);
    assert_eq!(info["Info"]["height"], 600.);
    let (_, pixels) = helper.request(json!({"Render": {"page": 1, "scale": 4096, "x": 0, "y": 0}}));
    assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
    assert_eq!(&pixels[450 * 4..450 * 4 + 4], &[0, 0, 255, 255]);
}

#[test]
fn native_worker_opens_ten_thousand_pages_lazily_and_rejects_bad_requests() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("many-pages.pdf");
    fixture(&path, 10_000, false);
    let mut helper = Helper::new(&path);
    let (info, _) = helper.request(json!({"Info": {"page": 10_000}}));
    assert_eq!(info["Info"]["pages"], 10_000);
    let (_, pixels) =
        helper.request(json!({"Render": {"page": 10_000, "scale": 16384, "x": 0, "y": 0}}));
    assert_eq!(pixels.len(), 512 * 512 * 4);
    let (error, pixels) =
        helper.request(json!({"Render": {"page": 1, "scale": 999999, "x": 0, "y": 0}}));
    assert!(error["Error"].is_string());
    assert!(pixels.is_empty());
    let bad = dir.path().join("bad.pdf");
    fs::write(&bad, b"not a PDF").unwrap();
    let mut helper = Helper::new(&bad);
    let (error, _) = helper.request(json!({"Info": {"page": 1}}));
    assert!(error["Error"].is_string());
    let mut helper = Helper::new(&path);
    let (info, _) = helper.request(json!({"Info": {"page": 1}}));
    assert_eq!(
        info["Info"]["pages"], 10_000,
        "worker can recover after a document error"
    );
}
