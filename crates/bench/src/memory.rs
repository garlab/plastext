//! Sample this process and all its helpers without exposing command lines.
use serde_json::{Value, json};
use std::{collections::HashSet, io, process::Command, sync::mpsc, thread, time::Duration};

pub struct Sampler {
    stop: mpsc::Sender<()>,
    thread: Option<thread::JoinHandle<io::Result<Value>>>,
}

impl Sampler {
    pub fn start() -> io::Result<Self> {
        let initial = snapshot(std::process::id())?;
        let (stop, receive) = mpsc::channel();
        let thread = thread::spawn(move || {
            let mut parent = vec![initial.0];
            let mut children = vec![initial.1];
            while receive
                .recv_timeout(Duration::from_millis(100))
                .is_err_and(|err| err == mpsc::RecvTimeoutError::Timeout)
            {
                let (p, c) = snapshot(std::process::id())?;
                parent.push(p);
                children.push(c);
            }
            // A fast workload can finish between periodic samples. Capture its
            // retained helper memory before the broker's idle timeout expires.
            let (p, c) = snapshot(std::process::id())?;
            parent.push(p);
            children.push(c);
            let summarize = |samples: Vec<f64>| {
                json!({
                    "max": samples.iter().copied().reduce(f64::max), "last": samples.last(), "samples": samples,
                })
            };
            Ok(
                json!({"interval_ms":100,"final_sample":true,"parent_mib":summarize(parent),"helpers_mib":summarize(children)}),
            )
        });
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
    pub fn finish(mut self) -> io::Result<Value> {
        self.stop.send(()).ok();
        self.thread
            .take()
            .unwrap()
            .join()
            .map_err(|_| io::Error::other("Memory sampler panicked"))?
    }
}
impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop.send(()).ok();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn snapshot(root: u32) -> io::Result<(f64, f64)> {
    let child = Command::new("ps")
        .args(["-axo", "pid=,ppid=,rss="])
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    let sampler_pid = child.id();
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(io::Error::other("Cannot sample process-tree RSS"));
    }
    let rows: Vec<(u32, u32, u64)> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((
                fields.next()?.parse().ok()?,
                fields.next()?.parse().ok()?,
                fields.next()?.parse().ok()?,
            ))
        })
        .collect();
    let parent = rows
        .iter()
        .find(|row| row.0 == root)
        .ok_or_else(|| io::Error::other("Benchmark process missing from RSS sample"))?
        .2;
    let mut descendants = HashSet::from([root]);
    loop {
        let before = descendants.len();
        for &(pid, ppid, _) in &rows {
            if pid != sampler_pid && descendants.contains(&ppid) {
                descendants.insert(pid);
            }
        }
        if before == descendants.len() {
            break;
        }
    }
    let children: u64 = rows
        .iter()
        .filter(|row| row.0 != root && descendants.contains(&row.0))
        .map(|row| row.2)
        .sum();
    Ok((parent as f64 / 1024., children as f64 / 1024.))
}
