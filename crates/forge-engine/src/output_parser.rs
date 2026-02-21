use forge_types::OutputAnalysis;
use serde_json::Value;

pub struct OutputParser;

impl OutputParser {
    pub fn parse(stdout: &str, stderr: &str, indicators: &[String]) -> OutputAnalysis {
        let text = format!("{stdout}\n{stderr}");
        let lowercase = text.to_ascii_lowercase();

        let mut completion_count = count_completion_indicators(&text, indicators);
        let exit_signal_true = lowercase.contains("exit_signal: true");
        let has_error = detect_error(&lowercase);
        let has_progress_hint = detect_progress_hint(&lowercase);

        let mut session_id = None;
        for line in stdout.lines() {
            if let Ok(value) = serde_json::from_str::<Value>(line) {
                if session_id.is_none() {
                    session_id = extract_session_id(&value);
                }
                if completion_count == 0 {
                    completion_count = count_json_indicators(&value, indicators);
                }
            }
        }

        OutputAnalysis {
            exit_signal_true,
            completion_indicators: completion_count,
            has_error,
            has_progress_hint,
            session_id,
        }
    }
}

fn count_completion_indicators(text: &str, indicators: &[String]) -> u32 {
    indicators
        .iter()
        .filter(|item| text.contains(*item))
        .count() as u32
}

fn detect_error(lowercase: &str) -> bool {
    lowercase.contains("\"error\"") || lowercase.contains("error:")
}

fn detect_progress_hint(lowercase: &str) -> bool {
    lowercase.contains("apply_patch")
        || lowercase.contains("updated file")
        || lowercase.contains("wrote")
        || lowercase.contains("created")
        || lowercase.contains("modified")
}

fn count_json_indicators(value: &Value, indicators: &[String]) -> u32 {
    indicators
        .iter()
        .filter(|needle| json_contains_string(value, needle))
        .count() as u32
}

fn extract_session_id(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in ["session_id", "thread_id", "conversation_id", "id"] {
                if let Some(Value::String(v)) = map.get(key) {
                    return Some(v.clone());
                }
            }
            map.values().find_map(extract_session_id)
        }
        Value::Array(arr) => arr.iter().find_map(extract_session_id),
        _ => None,
    }
}

fn json_contains_string(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(s) => s.contains(needle),
        Value::Array(arr) => arr.iter().any(|v| json_contains_string(v, needle)),
        Value::Object(map) => map.values().any(|v| json_contains_string(v, needle)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_exit_signal_true() {
        let analysis = OutputParser::parse("EXIT_SIGNAL: true\nSTATUS: COMPLETE", "", &[]);
        assert!(analysis.exit_signal_true);
    }

    #[test]
    fn exit_signal_case_insensitive() {
        let analysis = OutputParser::parse("exit_signal: TRUE", "", &[]);
        assert!(analysis.exit_signal_true);
    }

    #[test]
    fn no_exit_signal_when_false() {
        let analysis = OutputParser::parse("exit_signal: false", "", &[]);
        assert!(!analysis.exit_signal_true);
    }

    #[test]
    fn detects_completion_indicators() {
        let indicators = vec!["STATUS: COMPLETE".to_string(), "TASK_COMPLETE".to_string()];
        let analysis = OutputParser::parse("STATUS: COMPLETE\nTASK_COMPLETE", "", &indicators);
        assert_eq!(analysis.completion_indicators, 2);
    }

    #[test]
    fn detects_error_from_string() {
        let analysis = OutputParser::parse("Error: something failed", "", &[]);
        assert!(analysis.has_error);
    }

    #[test]
    fn detects_error_from_json() {
        let analysis = OutputParser::parse("{\"error\": \"failed\"}", "", &[]);
        assert!(analysis.has_error);
    }

    #[test]
    fn detects_progress_hint_apply_patch() {
        let analysis = OutputParser::parse("apply_patch to file", "", &[]);
        assert!(analysis.has_progress_hint);
    }

    #[test]
    fn detects_progress_hint_updated_file() {
        let analysis = OutputParser::parse("Updated file src/main.rs", "", &[]);
        assert!(analysis.has_progress_hint);
    }

    #[test]
    fn detects_progress_hint_wrote() {
        let analysis = OutputParser::parse("wrote 5 lines", "", &[]);
        assert!(analysis.has_progress_hint);
    }

    #[test]
    fn detects_progress_hint_created() {
        let analysis = OutputParser::parse("Created new module", "", &[]);
        assert!(analysis.has_progress_hint);
    }

    #[test]
    fn detects_progress_hint_modified() {
        let analysis = OutputParser::parse("Modified config", "", &[]);
        assert!(analysis.has_progress_hint);
    }

    #[test]
    fn extracts_session_id_from_json() {
        let json = r#"{"type":"thread.started","thread_id":"abc123"}"#;
        let analysis = OutputParser::parse(json, "", &[]);
        assert_eq!(analysis.session_id, Some("abc123".to_string()));
    }

    #[test]
    fn extracts_session_id_from_nested_json() {
        let json = r#"{"event":{"session_id":"xyz789"}}"#;
        let analysis = OutputParser::parse(json, "", &[]);
        assert_eq!(analysis.session_id, Some("xyz789".to_string()));
    }

    #[test]
    fn combines_stdout_and_stderr() {
        let analysis = OutputParser::parse("stdout output", "error: from stderr", &[]);
        assert!(analysis.has_error);
    }

    #[test]
    fn counts_json_indicators() {
        let indicators = vec!["COMPLETE".to_string()];
        let json = r#"{"status": "COMPLETE", "result": {"state": "COMPLETE"}}"#;
        let analysis = OutputParser::parse(json, "", &indicators);
        assert_eq!(analysis.completion_indicators, 1);
    }

    #[test]
    fn no_progress_when_no_hints() {
        let analysis = OutputParser::parse("just thinking...", "", &[]);
        assert!(!analysis.has_progress_hint);
    }

    #[test]
    fn empty_output_no_false_positives() {
        let analysis = OutputParser::parse("", "", &[]);
        assert!(!analysis.exit_signal_true);
        assert!(!analysis.has_error);
        assert!(!analysis.has_progress_hint);
        assert_eq!(analysis.completion_indicators, 0);
        assert!(analysis.session_id.is_none());
    }
}
