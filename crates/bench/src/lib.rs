//! Extreme-case fixture generators and benchmarks (spec §6, §9).
//!
//! The generators write the files of the spec §6.2 table deterministically,
//! so runs are comparable across machines and commits.

use std::{
    fs,
    io::{self, Write},
    path::Path,
};

/// A small deterministic generator (xorshift64), so fixtures are reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// Writes about `size` bytes of server-log lines of around 100 bytes each.
pub fn write_log(out: &mut impl Write, size: u64) -> io::Result<()> {
    const LEVELS: [&str; 4] = ["INFO ", "INFO ", "WARN ", "DEBUG"];
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let mut written = 0;
    let mut line = Vec::with_capacity(128);
    let mut index: u64 = 0;
    while written < size {
        let r = rng.next();
        line.clear();
        writeln!(
            line,
            "2026-10-24T{:02}:{:02}:{:02}.{:03}Z {} request id={:08x} path=/api/v1/items/{} status={} duration_ms={}",
            (index / 3_600_000) % 24,
            (index / 60_000) % 60,
            (index / 1000) % 60,
            index % 1000,
            LEVELS[(r % 4) as usize],
            r >> 32,
            r % 9973,
            if r.is_multiple_of(50) { 500 } else { 200 },
            r % 487,
        )?;
        out.write_all(&line)?;
        written += line.len() as u64;
        index += 1;
    }
    Ok(())
}

/// Writes a single line of `size` bytes of minified JSON, without a newline.
pub fn write_long_line(out: &mut impl Write, size: u64) -> io::Result<()> {
    if size < 2 {
        return out.write_all(&b"[]"[..size as usize]);
    }
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut item = Vec::with_capacity(96);
    out.write_all(b"[")?;
    let mut written = 1;
    while written + 1 < size {
        let r = rng.next();
        item.clear();
        write!(
            item,
            r#"{{"id":{},"name":"item-{:x}","tags":["a","b"]}},"#,
            r % 100_000,
            r >> 40
        )?;
        let take = item.len().min((size - written - 1) as usize);
        out.write_all(&item[..take])?;
        written += take as u64;
    }
    out.write_all(b"]")
}

/// Creates `count` empty files directly in `dir`.
pub fn create_wide_dir(dir: &Path, count: u64) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    for i in 0..count {
        fs::File::create(dir.join(format!("file-{i:07}.txt")))?;
    }
    Ok(())
}

/// About `size` bytes of log lines, for in-memory benchmarks.
pub fn log_bytes(size: u64) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(size as usize + 128);
    write_log(&mut bytes, size).expect("writing to a Vec can't fail");
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generators_are_deterministic_and_sized() {
        let log = log_bytes(10_000);
        assert_eq!(log, log_bytes(10_000));
        assert!((10_000..10_200).contains(&log.len()));
        assert!(log.ends_with(b"\n"));

        let mut line = Vec::new();
        write_long_line(&mut line, 5000).unwrap();
        assert_eq!(line.len(), 5000);
        assert!(!line.contains(&b'\n'));
    }
}
