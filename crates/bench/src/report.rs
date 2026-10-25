//! Commit-sized summaries and separate, local-only diagnostic artifacts.
use serde_json::{Value, json};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub struct LocalRun {
    directory: PathBuf,
}

impl LocalRun {
    pub fn new(kind: &str) -> io::Result<Self> {
        let root = Path::new("target/benchmarks");
        fs::create_dir_all(root)?;
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let directory = root.join(format!("{kind}-{time}-{}", std::process::id()));
        // Fail rather than overwrite a previous run, even on a clock collision.
        fs::create_dir(&directory)?;
        Ok(Self { directory })
    }

    pub fn log(&self) -> PathBuf {
        self.directory.join("frames.bench-raw.log")
    }

    pub fn save(&self, report: &Value, output: Option<&Path>) -> io::Result<Value> {
        let raw = self.directory.join("report.bench-raw.json");
        fs::write(&raw, serde_json::to_vec_pretty(report)?)?;
        let summary = summary(report);
        if let Some(output) = output {
            fs::write(output, serde_json::to_vec_pretty(&summary)?)?;
        }
        eprintln!("Local benchmark data: {}", self.directory.display());
        Ok(summary)
    }
}

/// No sample arrays, stderr/log paths, absolute fixture paths or diagnostic
/// text in the shareable output. Preserve the recorded statistics and outcome.
fn summary(report: &Value) -> Value {
    fn compact(value: &Value) -> Value {
        match value {
            Value::Object(fields) => {
                let mut result = serde_json::Map::new();
                for (key, value) in fields {
                    match key.as_str() {
                        "samples" => {
                            result.insert(
                                "sample_count".into(),
                                json!(value.as_array().map_or(0, Vec::len)),
                            );
                        }
                        "warnings" => {
                            result.insert(
                                "warning_count".into(),
                                json!(value.as_array().map_or(0, Vec::len)),
                            );
                        }
                        "log" => {}
                        "file" | "source" if value.is_string() => {
                            let name = Path::new(value.as_str().unwrap())
                                .file_name()
                                .unwrap_or_default();
                            result.insert(key.clone(), json!(name.to_string_lossy()));
                        }
                        _ => {
                            result.insert(key.clone(), compact(value));
                        }
                    }
                }
                Value::Object(result)
            }
            Value::Array(values) => Value::Array(values.iter().map(compact).collect()),
            other => other.clone(),
        }
    }
    let mut result = compact(report);
    result["schema"] = json!(2);
    result["artifact"] = json!("summary");
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_samples_and_private_paths_never_reach_the_summary() {
        let raw = json!({
            "schema": 1, "file": "/Users/private/fixture.md", "log": "/private/tmp/session.log",
            "passed": false, "complete": false, "observed_frames": 3, "requested_frames": 10000,
            "warnings": ["diagnostic with /Users/private/details"],
            "metrics": [{"name":"draw", "samples":[1.0,2.0,30.0], "p50":2.0,"p99":30.0,"max":30.0,"budget":8.0,"pass":false}]
        });
        let result = summary(&raw);
        assert_eq!(result["artifact"], "summary");
        assert_eq!(result["metrics"][0]["sample_count"], 3);
        for key in ["p50", "p99", "max", "budget", "pass"] {
            assert_eq!(result["metrics"][0][key], raw["metrics"][0][key]);
        }
        assert_eq!(result["file"], "fixture.md");
        assert_eq!(result["complete"], false);
        assert_eq!(result["passed"], false);
        assert_eq!(result["warning_count"], 1);
        let encoded = result.to_string();
        for forbidden in ["samples", "/Users", "/private", "session.log", "diagnostic"] {
            assert!(!encoded.contains(forbidden), "{forbidden}");
        }
        assert_eq!(raw["metrics"][0]["samples"].as_array().unwrap().len(), 3);
    }
}
