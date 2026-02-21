use anyhow::{Context, Result};
use chrono::Local;
use serde::de::DeserializeOwned;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

pub fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let body = serde_json::to_string_pretty(value).context("failed to serialize json")?;
    fs::write(path, body).with_context(|| format!("failed to write {}", path.display()))
}

pub fn read_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> T {
    match fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => T::default(),
    }
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse json from {}", path.display()))
}

pub fn append_history(path: &Path, line: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let stamped = stamp_lines(line);
    file.write_all(stamped.as_bytes())
        .with_context(|| format!("failed to append {}", path.display()))
}

pub fn append_live_activity(path: &Path, text: &str) -> Result<()> {
    let payload = serde_json::json!({
        "item": {
            "type": "agent_message",
            "text": text,
        }
    });
    append_history(path, &format!("{}\n", serde_json::to_string(&payload)?))
}

pub fn stamp_lines(input: &str) -> String {
    let ts = Local::now().format("%H:%M:%S").to_string();
    let mut out = String::new();
    for segment in input.split_inclusive('\n') {
        let has_newline = segment.ends_with('\n');
        let content = segment.trim_end_matches('\n');
        if content.is_empty() {
            continue;
        }
        out.push_str(&format!("[{}] {}", ts, content));
        if has_newline {
            out.push('\n');
        }
    }
    if out.is_empty() && !input.trim().is_empty() {
        out.push_str(&format!("[{}] {}", ts, input.trim()));
    }
    out
}

pub fn read_lines_reverse(path: &Path, limit: usize) -> Result<Vec<String>> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut lines: Vec<String> = reader
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.trim().is_empty())
        .collect();
    lines.reverse();
    lines.truncate(limit);
    Ok(lines)
}

pub fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("failed to create directory {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use tempfile::tempdir;

    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
    struct TestData {
        name: String,
        value: i32,
    }

    #[test]
    fn write_and_read_json() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("test.json");
        let data = TestData {
            name: "test".to_string(),
            value: 42,
        };

        write_json(&path, &data).expect("write");
        let read: TestData = read_json(&path).expect("read");

        assert_eq!(read, data);
    }

    #[test]
    fn read_json_or_default_returns_default_when_missing() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("missing.json");

        let result: TestData = read_json_or_default(&path);
        assert_eq!(result.name, "");
        assert_eq!(result.value, 0);
    }

    #[test]
    fn append_history_creates_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("history.log");

        append_history(&path, "line 1\n").expect("append 1");
        append_history(&path, "line 2\n").expect("append 2");

        let content = fs::read_to_string(&path).expect("read");
        assert!(content.contains("line 1"));
        assert!(content.contains("line 2"));
    }

    #[test]
    fn stamp_lines_adds_timestamp() {
        let result = stamp_lines("hello world\n");
        assert!(result.contains("["));
        assert!(result.contains("]"));
        assert!(result.contains("hello world"));
    }

    #[test]
    fn stamp_lines_handles_multiple_lines() {
        let result = stamp_lines("line1\nline2\n");
        assert!(result.contains("line1"));
        assert!(result.contains("line2"));
    }

    #[test]
    fn stamp_lines_ignores_empty_lines() {
        let result = stamp_lines("\n\nhello\n\n");
        assert!(result.contains("hello"));
    }

    #[test]
    fn append_live_activity_creates_json() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("live.log");

        append_live_activity(&path, "test activity").expect("append");

        let content = fs::read_to_string(&path).expect("read");
        assert!(content.contains("agent_message"));
        assert!(content.contains("test activity"));
    }

    #[test]
    fn read_lines_reverse_returns_reversed() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("lines.txt");
        fs::write(&path, "line1\nline2\nline3\n").expect("write");

        let lines = read_lines_reverse(&path, 10).expect("read");

        assert_eq!(lines, vec!["line3", "line2", "line1"]);
    }

    #[test]
    fn read_lines_reverse_respects_limit() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("lines.txt");
        fs::write(&path, "line1\nline2\nline3\n").expect("write");

        let lines = read_lines_reverse(&path, 2).expect("read");

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "line3");
        assert_eq!(lines[1], "line2");
    }

    #[test]
    fn ensure_dir_creates_directory() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("dir");

        ensure_dir(&path).expect("ensure");

        assert!(path.exists());
        assert!(path.is_dir());
    }
}
