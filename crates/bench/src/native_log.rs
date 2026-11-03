//! Interpret completion markers independently of process exit status. A clean
//! window close or a partial parse must never certify a scrolling benchmark.
pub struct NativeLog {
    pub frames: Vec<f64>,
    pub intervals: Vec<f64>,
    pub paints: Vec<(&'static str, f64, Option<f64>)>,
    finished: bool,
    first: bool,
    content: bool,
    full: bool,
    indexed: bool,
    table: bool,
    cache: std::collections::HashMap<&'static str, usize>,
}

impl NativeLog {
    pub fn parse(stderr: &str, preview: bool) -> Self {
        let mut result = Self {
            frames: Vec::new(),
            intervals: Vec::new(),
            paints: Vec::new(),
            finished: false,
            first: false,
            content: false,
            full: !preview,
            indexed: false,
            table: false,
            cache: std::collections::HashMap::new(),
        };
        let mut scrolling = false;
        for line in stderr.lines() {
            match line {
                "reading: indexed preview" => result.indexed = true,
                "reading: csv preview" => result.table = true,
                "reading: scrolling" => scrolling = true,
                "reading: done" => {
                    result.finished = true;
                    scrolling = false;
                }
                _ => {}
            }
            for (prefix, key) in [
                ("reading: index bytes ", "index_bytes"),
                ("reading: parsed cache bytes ", "parsed_cache_bytes"),
                ("reading: placeholder frames ", "placeholder_frames"),
                ("reading: visited targets ", "visited_targets"),
                ("reading: required targets ", "required_targets"),
                ("reading: page loads ", "page_loads"),
                ("reading: page reloads ", "page_reloads"),
                ("reading: cached pages ", "cached_pages"),
                ("reading: cached rows ", "cached_rows"),
                ("reading: scrolled frames ", "scrolled_frames"),
            ] {
                if let Some(value) = line.strip_prefix(prefix).and_then(|s| s.parse().ok()) {
                    result.cache.insert(key, value);
                }
            }
            for (prefix, budget) in [
                ("reading: first paint ", Some(50.)),
                ("reading: syntax paint ", Some(100.)),
                ("reading: full syntax paint ", None),
                ("reading: preview paint ", Some(100.)),
                ("reading: full preview paint ", None),
            ] {
                if let Some(value) = line.strip_prefix(prefix).and_then(duration_ms) {
                    result.paints.push((prefix.trim(), value, budget));
                    result.first |= prefix == "reading: first paint ";
                    result.content |= prefix
                        == if preview {
                            "reading: preview paint "
                        } else {
                            "reading: syntax paint "
                        };
                    result.full |= preview && prefix == "reading: full preview paint ";
                }
            }
            if scrolling
                && let Some(value) = line.strip_prefix("frame duration: ").and_then(duration_ms)
            {
                result.frames.push(value);
            }
            if scrolling
                && let Some(value) = line
                    .strip_prefix("reading: frame interval ")
                    .and_then(duration_ms)
            {
                result.intervals.push(value);
            }
        }
        result
    }

    pub fn complete(&self, success: bool, timed_out: bool, frames: usize) -> bool {
        success
            && !timed_out
            && self.finished
            && self.first
            && self.content
            && self.full
            && self.frames.len() >= frames.max(1)
            && (!self.table
                || (self.cache.get("visited_targets").is_some_and(|&n| n > 0)
                    && self.cache.get("visited_targets") == self.cache.get("required_targets")
                    && self.cache.get("cached_pages").is_some_and(|&n| n <= 9)
                    && self.cache.get("cached_rows").is_some_and(|&n| n <= 9 * 64)
                    && self.cache.get("scrolled_frames").is_some_and(|&n| {
                        n >= frames.max(1).div_ceil(2) && n <= self.frames.len()
                    })))
            && (!self.indexed
                || (self.cache.get("visited_targets").is_some_and(|&n| n > 0)
                    && self.cache.get("visited_targets") == self.cache.get("required_targets")
                    && self
                        .cache
                        .get("parsed_cache_bytes")
                        .is_some_and(|&n| n <= plastext_markdown::PARSED_CACHE_BYTES)))
    }

    pub fn cache_summary(&self) -> serde_json::Value {
        if self.indexed || self.table {
            serde_json::json!(self.cache)
        } else {
            serde_json::Value::Null
        }
    }
}

fn duration_ms(value: &str) -> Option<f64> {
    for (suffix, multiplier) in [("ms", 1.), ("µs", 0.001), ("ns", 0.000001), ("s", 1000.)] {
        if let Some(number) = value.strip_suffix(suffix) {
            let value = number.trim().parse::<f64>().ok()? * multiplier;
            return (value.is_finite() && value >= 0.).then_some(value);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    const LOG: &str = "frame duration: 99ms\nreading: first paint 12 ms\nreading: preview paint 15 ms\nreading: scrolling\nframe duration: 2ms\nreading: full preview paint 2500 ms\nframe duration: 2500µs\nreading: done\nframe duration: 99ms\n";
    #[test]
    fn counts_only_scroll_frames_and_requires_complete_markdown() {
        let log = NativeLog::parse(LOG, true);
        assert_eq!(log.frames, [2., 2.5]);
        assert!(log.complete(true, false, 2));
        assert!(!log.complete(true, false, 3));
        assert!(!log.complete(false, false, 2));
        assert!(!log.complete(true, true, 2));
        for missing in [
            "reading: done",
            "reading: full preview paint 2500 ms",
            "reading: first paint 12 ms",
        ] {
            assert!(!NativeLog::parse(&LOG.replace(missing, ""), true).complete(true, false, 2));
        }
        assert!(!NativeLog::parse(LOG, false).complete(true, false, 2));
    }
    #[test]
    fn rejects_invalid_timing_and_keeps_code_profile_semantics() {
        let code = LOG.replace("preview paint", "syntax paint");
        assert!(NativeLog::parse(&code, false).complete(true, false, 2));
        let invalid = LOG.replace("12 ms", "NaN ms");
        assert!(!NativeLog::parse(&invalid, true).complete(true, false, 2));
        for invalid in ["NaNms", "infms", "-1ms", "12minutes"] {
            assert!(duration_ms(invalid).is_none());
        }
    }

    #[test]
    fn indexed_preview_requires_real_target_content_and_bounded_cache() {
        let log = format!(
            "{LOG}reading: indexed preview\nreading: index bytes 1234\nreading: parsed cache bytes 1024\nreading: placeholder frames 2\nreading: visited targets 5\nreading: required targets 5\n"
        );
        assert!(NativeLog::parse(&log, true).complete(true, false, 2));
        assert!(
            !NativeLog::parse(&log.replace("visited targets 5", "visited targets 4"), true)
                .complete(true, false, 2)
        );
        assert!(
            !NativeLog::parse(
                &log.replace("cache bytes 1024", "cache bytes 999999999"),
                true
            )
            .complete(true, false, 2)
        );
        assert!(
            !NativeLog::parse(&log.replace("reading: parsed cache bytes 1024\n", ""), true)
                .complete(true, false, 2)
        );
    }

    #[test]
    fn csv_requires_real_targets_and_bounded_row_cache() {
        let log = format!(
            "{LOG}reading: csv preview\nreading: cached pages 9\nreading: cached rows 576\nreading: visited targets 3\nreading: required targets 3\nreading: scrolled frames 1\n"
        );
        assert!(NativeLog::parse(&log, true).complete(true, false, 2));
        for invalid in [
            log.replace("cached pages 9", "cached pages 10"),
            log.replace("cached rows 576", "cached rows 577"),
            log.replace("visited targets 3", "visited targets 2"),
            log.replace("scrolled frames 1", "scrolled frames 0"),
            log.replace("scrolled frames 1", "scrolled frames 3"),
            log.replace("reading: scrolled frames 1\n", ""),
            log.replace("reading: cached pages 9\n", ""),
            log.replace("reading: full preview paint 2500 ms\n", ""),
        ] {
            assert!(!NativeLog::parse(&invalid, true).complete(true, false, 2));
        }
    }
}
