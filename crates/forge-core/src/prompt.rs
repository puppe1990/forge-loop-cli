use std::fs;
use std::path::Path;

use crate::status::read_json_or_default;
use forge_types::ProgressSnapshot;

pub fn build_plan_prompt(cwd: &Path) -> Option<String> {
    let plan_file = cwd.join(".forge/plan.md");
    let plan = fs::read_to_string(plan_file).ok()?;
    let trimmed = plan.trim();
    if trimmed.is_empty() {
        return None;
    }

    let unchecked: Vec<String> = trimmed
        .lines()
        .filter(|line| line.contains("- [ ]"))
        .map(|line| line.trim().to_string())
        .take(80)
        .collect();

    let pending_block = if unchecked.is_empty() {
        "No explicit unchecked checklist items found; continue from current repo state and finalize remaining plan work.".to_string()
    } else {
        format!(
            "Unchecked checklist items (execute only what is still pending):\n{}",
            unchecked.join("\n")
        )
    };

    let last_summary =
        read_json_or_default::<ProgressSnapshot>(&cwd.join(".forge/progress.json")).last_summary;
    let continuity = if last_summary.trim().is_empty() {
        "Last loop summary: (none)".to_string()
    } else {
        format!("Last loop summary: {}", last_summary.trim())
    };

    Some(format!(
        "You are continuing an iterative execution loop.\n\
Continue from current workspace state. Do NOT redo completed checklist items.\n\
Avoid broad scans like `rg --files`; inspect only files needed for the current pending task.\n\
Apply small, verifiable steps and run only targeted validations per step.\n\
Emit `EXIT_SIGNAL: true` only when all pending checklist items are complete.\n\n\
{continuity}\n\n\
{pending_block}\n\n\
Plan source: .forge/plan.md"
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanSummary {
    pub total_items: usize,
    pub unchecked_items: usize,
    pub checked_items: usize,
}

pub fn analyze_plan(cwd: &Path) -> Option<PlanSummary> {
    let plan_file = cwd.join(".forge/plan.md");
    let plan = fs::read_to_string(plan_file).ok()?;
    let trimmed = plan.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lines: Vec<&str> = trimmed.lines().collect();
    let unchecked = lines.iter().filter(|l| l.contains("- [ ]")).count();
    let checked = lines.iter().filter(|l| l.contains("- [x]")).count();

    Some(PlanSummary {
        total_items: unchecked + checked,
        unchecked_items: unchecked,
        checked_items: checked,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn returns_none_when_no_plan() {
        let dir = tempdir().expect("tempdir");
        let result = build_plan_prompt(dir.path());
        assert!(result.is_none());
    }

    #[test]
    fn returns_none_when_plan_empty() {
        let dir = tempdir().expect("tempdir");
        let forge_dir = dir.path().join(".forge");
        fs::create_dir_all(&forge_dir).expect("create .forge");
        fs::write(forge_dir.join("plan.md"), "   \n").expect("write empty plan");

        let result = build_plan_prompt(dir.path());
        assert!(result.is_none());
    }

    #[test]
    fn includes_unchecked_items() {
        let dir = tempdir().expect("tempdir");
        let forge_dir = dir.path().join(".forge");
        fs::create_dir_all(&forge_dir).expect("create .forge");
        fs::write(
            forge_dir.join("plan.md"),
            "# Plan\n- [ ] Task A\n- [x] Task B\n",
        )
        .expect("write plan");
        fs::write(
            forge_dir.join("progress.json"),
            r#"{"last_summary":"finished task B"}"#,
        )
        .expect("write progress");

        let prompt = build_plan_prompt(dir.path()).expect("prompt");

        assert!(prompt.contains("continuing an iterative execution loop"));
        assert!(prompt.contains("Do NOT redo completed checklist items"));
        assert!(prompt.contains("Task A"));
        assert!(!prompt.contains("Task B"));
        assert!(prompt.contains("finished task B"));
    }

    #[test]
    fn includes_continuity_message_when_no_progress() {
        let dir = tempdir().expect("tempdir");
        let forge_dir = dir.path().join(".forge");
        fs::create_dir_all(&forge_dir).expect("create .forge");
        fs::write(forge_dir.join("plan.md"), "# Plan\n- [ ] Task A\n").expect("write plan");

        let prompt = build_plan_prompt(dir.path()).expect("prompt");

        assert!(prompt.contains("Last loop summary: (none)"));
    }

    #[test]
    fn analyze_plan_counts_items() {
        let dir = tempdir().expect("tempdir");
        let forge_dir = dir.path().join(".forge");
        fs::create_dir_all(&forge_dir).expect("create .forge");
        fs::write(
            forge_dir.join("plan.md"),
            "# Plan\n- [ ] Task A\n- [x] Task B\n- [ ] Task C\n",
        )
        .expect("write plan");

        let summary = analyze_plan(dir.path()).expect("summary");

        assert_eq!(summary.total_items, 3);
        assert_eq!(summary.unchecked_items, 2);
        assert_eq!(summary.checked_items, 1);
    }

    #[test]
    fn analyze_plan_returns_none_when_no_plan() {
        let dir = tempdir().expect("tempdir");
        let result = analyze_plan(dir.path());
        assert!(result.is_none());
    }

    #[test]
    fn limits_unchecked_items_to_80() {
        let dir = tempdir().expect("tempdir");
        let forge_dir = dir.path().join(".forge");
        fs::create_dir_all(&forge_dir).expect("create .forge");

        let mut plan = String::from("# Plan\n");
        for i in 0..100 {
            plan.push_str(&format!("- [ ] Task {}\n", i));
        }
        fs::write(forge_dir.join("plan.md"), &plan).expect("write plan");

        let prompt = build_plan_prompt(dir.path()).expect("prompt");

        assert!(prompt.contains("Task 0"));
        assert!(prompt.contains("Task 79"));
        assert!(!prompt.contains("Task 80"));
    }
}
