use anyhow::{bail, Context, Result};
use chrono::{DateTime, Datelike, Local, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct SddInterview {
    pub project_name: String,
    pub product_goal: String,
    pub target_users: String,
    pub thinking: ThinkingMode,
    pub in_scope: String,
    pub out_of_scope: String,
    pub constraints: String,
    pub acceptance_criteria: String,
    pub scenarios: String,
    pub tests: String,
    pub max_loops: u64,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum, Serialize, Deserialize)]
pub enum ThinkingMode {
    Off,
    Summary,
    Raw,
}

impl From<ThinkingMode> for forge_config::ThinkingMode {
    fn from(value: ThinkingMode) -> Self {
        match value {
            ThinkingMode::Off => forge_config::ThinkingMode::Off,
            ThinkingMode::Summary => forge_config::ThinkingMode::Summary,
            ThinkingMode::Raw => forge_config::ThinkingMode::Raw,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SddMeta {
    pub id: String,
    pub project_name: String,
    pub goal: String,
    pub created_at_epoch: u64,
}

impl Default for SddMeta {
    fn default() -> Self {
        Self {
            id: String::new(),
            project_name: String::new(),
            goal: String::new(),
            created_at_epoch: 0,
        }
    }
}

pub fn sdd_root(cwd: &Path) -> std::path::PathBuf {
    cwd.join(".forge/sdds")
}

pub fn current_sdd_id(cwd: &Path) -> Result<Option<String>> {
    let path = cwd.join(".forge/current_sdd");
    if !path.exists() {
        return Ok(None);
    }
    let content =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let id = content.trim().to_string();
    if id.is_empty() {
        Ok(None)
    } else {
        Ok(Some(id))
    }
}

pub fn read_sdd_meta(cwd: &Path, id: &str) -> SddMeta {
    let meta_path = sdd_root(cwd).join(id).join("meta.json");
    if !meta_path.exists() {
        return SddMeta::default();
    }
    fs::read_to_string(&meta_path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn collect_sdd_answers() -> Result<SddInterview> {
    println!("[Phase 1] Intent");
    let project_name = ask("project name", "forge project")?;
    let product_goal = ask("primary goal", "deliver autonomous coding outcomes")?;
    let target_users = ask("target users", "developers")?;

    println!("\n[Phase 2] Scope");
    let in_scope = ask("in scope for this phase", "run, status, monitor")?;
    let out_of_scope = ask("out of scope", "setup, import, windows")?;

    println!("\n[Phase 3] Constraints");
    let constraints = ask(
        "technical constraints",
        "rust only, .forge runtime, .forgerc config",
    )?;

    println!("\n[Phase 4] Acceptance");
    let acceptance_criteria = ask(
        "acceptance criteria (semicolon-separated)",
        "dual exit gate works; status is consistent; monitor is stable",
    )?;

    println!("\n[Phase 5] Scenarios");
    let scenarios = ask(
        "given/when/then scenarios (semicolon-separated)",
        "Given completion+exit_signal true When run Then finish loop",
    )?;

    println!("\n[Phase 6] Testing");
    let tests = ask(
        "test strategy",
        "contract CLI tests, acceptance loop tests, resilience tests",
    )?;

    println!("\n[Phase 7] Reasoning");
    let thinking = ask_thinking("thinking mode", ThinkingMode::Summary)?;

    let max_loops_str = ask("max loops for execution", "100")?;
    let max_loops = max_loops_str.trim().parse::<u64>().unwrap_or(100);

    Ok(SddInterview {
        project_name,
        product_goal,
        target_users,
        thinking,
        in_scope,
        out_of_scope,
        constraints,
        acceptance_criteria,
        scenarios,
        tests,
        max_loops,
    })
}

pub fn create_sdd_snapshot(cwd: &Path, answers: &SddInterview) -> Result<String> {
    let forge_dir = cwd.join(".forge");
    let sdds_dir = sdd_root(cwd);
    let docs_dir = cwd.join("docs/specs/session");
    fs::create_dir_all(&forge_dir)?;
    fs::create_dir_all(&docs_dir)?;
    fs::create_dir_all(&sdds_dir)?;

    let spec = render_spec(answers);
    let acceptance = render_acceptance(answers);
    let scenarios = render_scenarios(answers);
    let plan = render_plan(answers);

    let id = format!(
        "{}-{}",
        epoch_now(),
        slugify(&answers.project_name, "session")
    );
    let snapshot_dir = sdds_dir.join(&id);
    fs::create_dir_all(&snapshot_dir)?;

    fs::write(snapshot_dir.join("spec.md"), &spec)?;
    fs::write(snapshot_dir.join("acceptance.md"), &acceptance)?;
    fs::write(snapshot_dir.join("scenarios.md"), &scenarios)?;
    fs::write(snapshot_dir.join("plan.md"), &plan)?;

    let meta = SddMeta {
        id: id.clone(),
        project_name: answers.project_name.clone(),
        goal: answers.product_goal.clone(),
        created_at_epoch: epoch_now(),
    };
    fs::write(
        snapshot_dir.join("meta.json"),
        serde_json::to_string_pretty(&meta)?,
    )?;

    Ok(id)
}

pub fn activate_sdd(cwd: &Path, id: &str) -> Result<()> {
    let source_dir = sdd_root(cwd).join(id);
    if !source_dir.exists() {
        bail!("sdd id not found: {}", id);
    }

    let forge_dir = cwd.join(".forge");
    let docs_dir = cwd.join("docs/specs/session");
    fs::create_dir_all(&forge_dir)?;
    fs::create_dir_all(&docs_dir)?;

    copy_required(source_dir.join("plan.md"), forge_dir.join("plan.md"))?;
    copy_required(source_dir.join("spec.md"), docs_dir.join("spec.md"))?;
    copy_required(
        source_dir.join("acceptance.md"),
        docs_dir.join("acceptance.md"),
    )?;
    copy_required(
        source_dir.join("scenarios.md"),
        docs_dir.join("scenarios.md"),
    )?;

    fs::write(forge_dir.join("current_sdd"), id)?;
    Ok(())
}

fn copy_required(from: std::path::PathBuf, to: std::path::PathBuf) -> Result<()> {
    let body =
        fs::read_to_string(&from).with_context(|| format!("failed to read {}", from.display()))?;
    fs::write(&to, body).with_context(|| format!("failed to write {}", to.display()))?;
    Ok(())
}

fn render_spec(a: &SddInterview) -> String {
    format!(
        "# Session Spec\n\n## Project\n{}\n\n## Goal\n{}\n\n## Target Users\n{}\n\n## In Scope\n{}\n\n## Out of Scope\n{}\n\n## Constraints\n{}\n",
        a.project_name, a.product_goal, a.target_users, a.in_scope, a.out_of_scope, a.constraints
    )
}

fn render_acceptance(a: &SddInterview) -> String {
    let mut out = String::from("# Session Acceptance Criteria\n\n");
    for item in a.acceptance_criteria.split(';') {
        let trimmed = item.trim();
        if !trimmed.is_empty() {
            out.push_str(&format!("- [ ] {}\n", trimmed));
        }
    }
    out.push_str("\n## Test Strategy\n");
    out.push_str(&a.tests);
    out.push('\n');
    out
}

fn render_scenarios(a: &SddInterview) -> String {
    let mut out = String::from("# Session Scenarios\n\n");
    for (idx, item) in a.scenarios.split(';').enumerate() {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        out.push_str(&format!("## Scenario {}\n{}\n\n", idx + 1, trimmed));
    }
    if out.trim() == "# Session Scenarios" {
        out.push_str(
            "## Scenario 1\nGiven a planned task When forge run executes Then progress is persisted\n",
        );
    }
    out
}

fn render_plan(a: &SddInterview) -> String {
    let epoch = epoch_now();

    format!(
        "# Execution Plan\n\nGenerated at epoch {}\n\n## Goal\n{}\n\n## Scope\n- In: {}\n- Out: {}\n\n## Constraints\n{}\n\n## Acceptance\n{}\n\n## Scenarios\n{}\n\n## Test Strategy\n{}\n\n## Thinking Mode\n{}\n\nExecute this plan incrementally. Only stop when completion indicators are present and EXIT_SIGNAL is true. Persist status and progress in .forge/.\n",
        epoch,
        a.product_goal,
        a.in_scope,
        a.out_of_scope,
        a.constraints,
        a.acceptance_criteria,
        a.scenarios,
        a.tests,
        match a.thinking {
            ThinkingMode::Off => "off",
            ThinkingMode::Summary => "summary",
            ThinkingMode::Raw => "raw",
        },
    )
}

fn ask(label: &str, default: &str) -> Result<String> {
    print!("- {} [{}]: ", label, default);
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let value = line.trim();
    if value.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(value.to_string())
    }
}

fn ask_thinking(label: &str, default: ThinkingMode) -> Result<ThinkingMode> {
    let default_str = match default {
        ThinkingMode::Off => "off",
        ThinkingMode::Summary => "summary",
        ThinkingMode::Raw => "raw",
    };
    loop {
        let value = ask(label, default_str)?;
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => return Ok(ThinkingMode::Off),
            "summary" => return Ok(ThinkingMode::Summary),
            "raw" => return Ok(ThinkingMode::Raw),
            _ => {
                println!("invalid thinking mode. use: off | summary | raw");
            }
        }
    }
}

fn slugify(input: &str, fallback: &str) -> String {
    let slug: String = input
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let slug = slug.split_whitespace().collect::<Vec<_>>().join("-");
    if slug.is_empty() {
        fallback.to_string()
    } else {
        slug
    }
}

fn epoch_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::from_secs(0))
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn sdd_root_returns_correct_path() {
        let dir = tempdir().expect("tempdir");
        let root = sdd_root(dir.path());
        assert!(root.ends_with(".forge/sdds"));
    }

    #[test]
    fn current_sdd_id_returns_none_when_missing() {
        let dir = tempdir().expect("tempdir");
        let result = current_sdd_id(dir.path()).expect("current_sdd_id");
        assert!(result.is_none());
    }

    #[test]
    fn current_sdd_id_returns_value_when_present() {
        let dir = tempdir().expect("tempdir");
        let forge_dir = dir.path().join(".forge");
        fs::create_dir_all(&forge_dir).expect("create forge dir");
        fs::write(forge_dir.join("current_sdd"), "test-sdd-123").expect("write current_sdd");

        let result = current_sdd_id(dir.path()).expect("current_sdd_id");
        assert_eq!(result, Some("test-sdd-123".to_string()));
    }

    #[test]
    fn read_sdd_meta_returns_default_when_missing() {
        let dir = tempdir().expect("tempdir");
        let meta = read_sdd_meta(dir.path(), "nonexistent");
        assert!(meta.id.is_empty());
    }

    #[test]
    fn read_sdd_meta_returns_parsed_value() {
        let dir = tempdir().expect("tempdir");
        let sdd_dir = sdd_root(dir.path()).join("test-sdd");
        fs::create_dir_all(&sdd_dir).expect("create sdd dir");
        fs::write(
            sdd_dir.join("meta.json"),
            r#"{"id":"test-sdd","project_name":"Test","goal":"Test goal","created_at_epoch":1000}"#,
        )
        .expect("write meta");

        let meta = read_sdd_meta(dir.path(), "test-sdd");
        assert_eq!(meta.id, "test-sdd");
        assert_eq!(meta.project_name, "Test");
    }

    #[test]
    fn activate_sdd_copies_files() {
        let dir = tempdir().expect("tempdir");
        let sdd_dir = sdd_root(dir.path()).join("test-sdd");
        fs::create_dir_all(&sdd_dir).expect("create sdd dir");
        fs::write(sdd_dir.join("plan.md"), "# plan").expect("write plan");
        fs::write(sdd_dir.join("spec.md"), "# spec").expect("write spec");
        fs::write(sdd_dir.join("acceptance.md"), "# acceptance").expect("write acceptance");
        fs::write(sdd_dir.join("scenarios.md"), "# scenarios").expect("write scenarios");

        activate_sdd(dir.path(), "test-sdd").expect("activate");

        assert!(dir.path().join(".forge/plan.md").exists());
        assert!(dir.path().join("docs/specs/session/spec.md").exists());
        assert_eq!(
            fs::read_to_string(dir.path().join(".forge/current_sdd")).expect("read current"),
            "test-sdd"
        );
    }

    #[test]
    fn activate_sdd_fails_for_unknown() {
        let dir = tempdir().expect("tempdir");
        let result = activate_sdd(dir.path(), "unknown");
        assert!(result.is_err());
    }

    #[test]
    fn render_spec_includes_all_fields() {
        let interview = SddInterview {
            project_name: "MyProject".to_string(),
            product_goal: "Build something".to_string(),
            target_users: "Developers".to_string(),
            thinking: ThinkingMode::Summary,
            in_scope: "Core features".to_string(),
            out_of_scope: "Extras".to_string(),
            constraints: "Time".to_string(),
            acceptance_criteria: "Works".to_string(),
            scenarios: "Scenario 1".to_string(),
            tests: "Unit tests".to_string(),
            max_loops: 10,
        };

        let spec = render_spec(&interview);

        assert!(spec.contains("MyProject"));
        assert!(spec.contains("Build something"));
        assert!(spec.contains("Developers"));
        assert!(spec.contains("Core features"));
    }

    #[test]
    fn render_acceptance_parses_semicolons() {
        let interview = SddInterview {
            project_name: "Test".to_string(),
            product_goal: "Goal".to_string(),
            target_users: "Users".to_string(),
            thinking: ThinkingMode::Summary,
            in_scope: "In".to_string(),
            out_of_scope: "Out".to_string(),
            constraints: "None".to_string(),
            acceptance_criteria: "Item 1; Item 2; Item 3".to_string(),
            scenarios: "Scenario".to_string(),
            tests: "Tests".to_string(),
            max_loops: 10,
        };

        let acceptance = render_acceptance(&interview);

        assert!(acceptance.contains("- [ ] Item 1"));
        assert!(acceptance.contains("- [ ] Item 2"));
        assert!(acceptance.contains("- [ ] Item 3"));
    }
}
