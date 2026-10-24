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
}

impl NativeLog {
    pub fn parse(stderr: &str, markdown: bool) -> Self {
        let mut result = Self {
            frames: Vec::new(),
            intervals: Vec::new(),
            paints: Vec::new(),
            finished: false,
            first: false,
            content: false,
            full: !markdown,
        };
        let mut scrolling = false;
        for line in stderr.lines() {
            match line {
                "reading: scrolling" => scrolling = true,
                "reading: done" => {
                    result.finished = true;
                    scrolling = false;
                }
                _ => {}
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
                        == if markdown {
                            "reading: preview paint "
                        } else {
                            "reading: syntax paint "
                        };
                    result.full |= markdown && prefix == "reading: full preview paint ";
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
}
