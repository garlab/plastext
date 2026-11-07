//! Clipboard text and source coordinates, computed from one captured version.
use super::*;
use std::{io, ops::ControlFlow};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Format {
    Text,
    Location,
    Lines,
    WithLocation,
}

pub(super) struct CopyTask {
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) _task: Task<()>,
}
impl Drop for CopyTask {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl TextView {
    pub fn set_file_path(&mut self, path: &std::path::Path) {
        self.file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
    }

    pub(super) fn copy_text(&mut self, format: Format, cx: &mut Context<Self>) {
        if self.has_pending_input() {
            self.notice = Some("Wait for pending input before copying.".into());
            cx.notify();
            return;
        }
        let Some(selection) = self.selection else {
            return;
        };
        let range = selection.range();
        if matches!(format, Format::Text | Format::Lines) && range.is_empty() {
            return;
        }
        let filename = self.file_name.clone().unwrap_or_default();
        if format != Format::Text && filename.is_empty() {
            return;
        }
        self.copy_task = None;
        let (source, index) = (self.source.clone(), self.index.clone());
        let cancel = Arc::new(AtomicBool::new(false));
        let work = cx.background_executor().spawn({
            let cancel = cancel.clone();
            async move { copied_text(&source, &index, &filename, range, format, &cancel) }
        });
        let current = cancel.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                if current.load(Ordering::Relaxed) {
                    return;
                }
                match result {
                    Ok(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                    Err(error) => this.notice = Some(format!("Couldn't copy: {error}").into()),
                }
                this.copy_task = None;
                cx.notify();
            })
            .ok();
        });
        self.copy_task = Some(CopyTask {
            cancel,
            _task: task,
        });
    }
}

fn check(cancel: &AtomicBool) -> io::Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(io::Error::new(io::ErrorKind::Interrupted, "copy cancelled"))
    } else {
        Ok(())
    }
}

fn cached<T>(
    source: &ViewSource,
    cancel: &AtomicBool,
    mut read: impl FnMut() -> Result<T, ReadError>,
) -> io::Result<T> {
    loop {
        check(cancel)?;
        match read() {
            Ok(value) => return Ok(value),
            Err(ReadError::Missing(range)) => source.load(range)?,
            Err(ReadError::Changed) => return Err(io::Error::other("the file changed")),
        }
    }
}

fn line_position(
    source: &ViewSource,
    index: &LineIndex,
    offset: u64,
    cancel: &AtomicBool,
) -> io::Result<(u64, u64)> {
    if let Some(line) = cached(source, cancel, || source.line_at(index, offset))?
        && let Some(start) = cached(source, cancel, || source.line_start(index, line))?
    {
        return Ok((line, start));
    }
    // Reader indexing may still be in progress. An explicit copy can scan on a
    // worker with bounded scratch space, without waiting or blocking the view.
    let (mut line, mut start, mut at) = (0, 0, 0);
    source.original.scan(0..offset, |chunk| {
        for newline in memchr::memchr_iter(b'\n', chunk) {
            line += 1;
            start = at + newline as u64 + 1;
        }
        at += chunk.len() as u64;
        if cancel.load(Ordering::Relaxed) {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })?;
    check(cancel)?;
    Ok((line, start))
}

fn read_text(source: &ViewSource, range: Range<u64>, cancel: &AtomicBool) -> io::Result<String> {
    if range.end - range.start > MAX_COPY {
        return Err(io::Error::other("text is too large to copy (over 64 MB)"));
    }
    let mut bytes = Vec::with_capacity((range.end - range.start) as usize);
    if let Some(snapshot) = &source.snapshot {
        let mut at = range.start;
        while at < range.end {
            check(cancel)?;
            let end = (at + 64 * 1024).min(range.end);
            snapshot.read_uncached(at..end, &mut bytes)?;
            at = end;
        }
    } else {
        source.original.scan(range, |chunk| {
            if cancel.load(Ordering::Relaxed) {
                return ControlFlow::Break(());
            }
            bytes.extend_from_slice(chunk);
            ControlFlow::Continue(())
        })?;
    }
    check(cancel)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn column(source: &ViewSource, start: u64, offset: u64, cancel: &AtomicBool) -> io::Result<u64> {
    if let Some(text) = &source.snapshot {
        let scalar = |at| {
            cached(source, cancel, || {
                text.byte_to_scalar(at).map_err(|error| match error {
                    plastext_buffer::TextError::Read(error) => error,
                    _ => ReadError::Changed,
                })
            })
        };
        Ok(scalar(offset)? - scalar(start)? + 1)
    } else {
        Ok(read_text(source, start..offset, cancel)?.chars().count() as u64 + 1)
    }
}

pub(super) fn copied_text(
    source: &ViewSource,
    index: &LineIndex,
    filename: &str,
    mut range: Range<u64>,
    format: Format,
    cancel: &AtomicBool,
) -> io::Result<String> {
    check(cancel)?;
    if range.start > range.end || range.end > source.len() {
        return Err(io::Error::other("selection is no longer valid"));
    }
    if format == Format::Text {
        return read_text(source, range, cancel);
    }
    let (start_line, line_start) = line_position(source, index, range.start, cancel)?;
    if format == Format::Location {
        return Ok(format!(
            "{filename}:{}:{}",
            start_line + 1,
            column(source, line_start, range.start, cancel)?
        ));
    }
    let current_line = range.is_empty();
    let end_line = if current_line {
        start_line
    } else {
        line_position(source, index, range.end - 1, cancel)?.0
    };
    let location = if end_line == start_line {
        format!("{filename}:{}", start_line + 1)
    } else {
        format!("{filename}:{}-{}", start_line + 1, end_line + 1)
    };
    if format == Format::Lines {
        return Ok(location);
    }
    if current_line {
        range.start = line_start;
        range.end = cached(source, cancel, || source.line_start(index, start_line + 1))?
            .unwrap_or(source.len());
        if source.snapshot.is_none() {
            let mut at = line_start;
            source.original.scan(
                line_start..range.end.min(line_start.saturating_add(MAX_COPY + 1)),
                |chunk| {
                    if cancel.load(Ordering::Relaxed) {
                        return ControlFlow::Break(());
                    }
                    if let Some(newline) = memchr::memchr(b'\n', chunk) {
                        range.end = at + newline as u64 + 1;
                        return ControlFlow::Break(());
                    }
                    at += chunk.len() as u64;
                    ControlFlow::Continue(())
                },
            )?;
        }
    }
    let mut text = read_text(source, range, cancel)?;
    if current_line {
        if text.ends_with('\n') {
            text.pop();
        }
        if text.ends_with('\r') {
            text.pop();
        }
    }
    let separator = if text.contains(['\r', '\n']) {
        "\n"
    } else {
        " "
    };
    Ok(format!("in {location}:{separator}{text}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use plastext_buffer::{EditBuffer, OpenOptions, UndoGroup};

    fn copied(text: &str, range: Range<u64>, format: Format, edited: bool) -> String {
        let original = Arc::new(FileSource::from_bytes(text.as_bytes().to_vec()));
        let index = LineIndex::build(&original).unwrap();
        let source = ViewSource {
            original,
            snapshot: edited.then(|| EditBuffer::from_bytes(text.as_bytes().to_vec()).snapshot()),
        };
        copied_text(
            &source,
            &index,
            "Café notes.md",
            range,
            format,
            &AtomicBool::new(false),
        )
        .unwrap()
    }

    #[test]
    fn locations_and_excerpts_use_logical_lines_and_unicode_columns() {
        for edited in [false, true] {
            for newline in ["\n", "\r\n"] {
                let first = format!("one{newline}");
                let second = format!("\té😀text{newline}");
                let text = format!("{first}{second}three");
                let start = first.len() as u64;
                let end = start + second.len() as u64;
                let offset = start + "\té😀".len() as u64;
                assert_eq!(
                    copied(&text, offset..offset, Format::Location, edited),
                    "Café notes.md:2:4"
                );
                assert_eq!(
                    copied(&text, start..end, Format::Lines, edited),
                    "Café notes.md:2"
                );
                assert_eq!(
                    copied(&text, 0..end, Format::Lines, edited),
                    "Café notes.md:1-2"
                );
                assert_eq!(
                    copied(&text, start..end, Format::WithLocation, edited),
                    format!("in Café notes.md:2:\n{second}")
                );
                assert_eq!(
                    copied(&text, offset..offset, Format::WithLocation, edited),
                    "in Café notes.md:2: \té😀text"
                );
            }
            assert_eq!(
                copied("one\n", 4..4, Format::Location, edited),
                "Café notes.md:2:1"
            );
            assert_eq!(
                copied("", 0..0, Format::WithLocation, edited),
                "in Café notes.md:1: "
            );
        }
        assert_eq!(
            copied("one\rtwo\rthree", 4..8, Format::Lines, true),
            "Café notes.md:2"
        );
    }

    #[test]
    fn captured_versions_keep_unsaved_text_and_positions_together() {
        let original = Arc::new(FileSource::from_bytes(b"original\n".to_vec()));
        let index = LineIndex::build(&original).unwrap();
        let mut buffer = EditBuffer::from_bytes(b"original\n".to_vec());
        buffer
            .replace_text(0..0, "unsaved\n", UndoGroup::New)
            .unwrap();
        let source = ViewSource {
            original,
            snapshot: Some(buffer.snapshot()),
        };
        buffer
            .replace_text(0..8, "changed\nagain\n", UndoGroup::New)
            .unwrap();
        assert_eq!(
            copied_text(
                &source,
                &index,
                "file.txt",
                0..7,
                Format::WithLocation,
                &AtomicBool::new(false)
            )
            .unwrap(),
            "in file.txt:1: unsaved"
        );
        assert_eq!(
            copied_text(
                &source,
                &index,
                "file.txt",
                8..8,
                Format::Location,
                &AtomicBool::new(false)
            )
            .unwrap(),
            "file.txt:2:1"
        );
        assert!(
            copied_text(
                &source,
                &index,
                "file.txt",
                0..1,
                Format::Text,
                &AtomicBool::new(true)
            )
            .is_err()
        );
    }

    #[test]
    fn copying_larger_than_the_reader_cache_streams_without_filling_it() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let bytes = "abc\n".repeat(600_000);
        std::fs::write(file.path(), &bytes).unwrap();
        let original = Arc::new(
            FileSource::open(
                file.path(),
                OpenOptions {
                    large_file_threshold: 1,
                    block_cache: 1024 * 1024,
                },
            )
            .unwrap(),
        );
        let index = LineIndex::build(&original).unwrap();
        let source = ViewSource {
            original,
            snapshot: None,
        };
        assert_eq!(
            copied_text(
                &source,
                &index,
                "large.txt",
                0..source.len(),
                Format::Text,
                &AtomicBool::new(false)
            )
            .unwrap(),
            bytes
        );
        assert!(source.missing(0..source.len()).is_some());
    }
}
