use anyhow::Result;
use forge_config::{load_run_config, CliOverrides, EngineKind};
use std::fs;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct DoctorCheck {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct DoctorReport {
    pub cwd: String,
    pub ok: bool,
    pub checks: Vec<DoctorCheck>,
    pub warnings: Vec<String>,
    pub attempted_fixes: Vec<String>,
}

pub fn run_doctor(cwd: &Path, fix: bool, strict: bool) -> Result<DoctorReport> {
    let before = collect_doctor_checks(cwd);
    let before_warnings = collect_doctor_warnings(cwd);
    let mut attempted_fixes = Vec::new();

    if fix {
        attempted_fixes = apply_doctor_fixes(cwd)?;
    }

    let checks = collect_doctor_checks(cwd);
    let failed = checks.iter().filter(|c| !c.ok).count();
    let warnings = collect_doctor_warnings(cwd);
    let strict_failed = strict && !warnings.is_empty();

    Ok(DoctorReport {
        cwd: cwd.display().to_string(),
        ok: failed == 0 && !strict_failed,
        checks,
        warnings,
        attempted_fixes,
    })
}

pub fn collect_doctor_checks(cwd: &Path) -> Vec<DoctorCheck> {
    let cfg = load_run_config(cwd, &CliOverrides::default()).ok();
    let engine = if let Some(ref c) = cfg {
        check_engine_available(c.engine, &c.engine_cmd)
    } else {
        check_engine_available(EngineKind::Codex, "codex")
    };
    let git = check_git_repo(cwd);
    let write = check_runtime_writable(cwd);
    let config = check_config_loadable(cwd);

    vec![
        DoctorCheck {
            name: "engine_available",
            ok: engine.0,
            detail: engine.1,
        },
        DoctorCheck {
            name: "git_repository",
            ok: git.0,
            detail: git.1,
        },
        DoctorCheck {
            name: "runtime_writable",
            ok: write.0,
            detail: write.1,
        },
        DoctorCheck {
            name: "config_loadable",
            ok: config.0,
            detail: config.1,
        },
    ]
}

pub fn collect_doctor_warnings(cwd: &Path) -> Vec<String> {
    let mut warnings = Vec::new();
    if !cwd.join(".forgerc").exists() {
        warnings.push("missing .forgerc (using only env/defaults)".to_string());
    }
    if !cwd
        .join(".forge")
        .join("analyze")
        .join("latest.json")
        .exists()
    {
        warnings.push("no persisted analyze report yet (.forge/analyze/latest.json)".to_string());
    }
    warnings
}

pub fn apply_doctor_fixes(cwd: &Path) -> Result<Vec<String>> {
    let mut fixes = Vec::new();

    let runtime_dir = cwd.join(".forge");
    if !runtime_dir.exists() {
        fs::create_dir_all(&runtime_dir)?;
        fixes.push("created .forge runtime directory".to_string());
    }

    let forgerc = cwd.join(".forgerc");
    if !forgerc.exists() {
        let template = "# forge defaults\nengine = \"codex\"\nmax_calls_per_hour = 100\ntimeout_minutes = 15\n";
        fs::write(&forgerc, template)?;
        fixes.push("created .forgerc with baseline defaults".to_string());
    }

    Ok(fixes)
}

fn check_engine_available(engine: EngineKind, cmd: &str) -> (bool, String) {
    match Command::new(cmd).arg("--version").output() {
        Ok(output) if output.status.success() => {
            let v = String::from_utf8_lossy(&output.stdout).trim().to_string();
            (
                true,
                if v.is_empty() {
                    format!("{} found", engine.as_str())
                } else {
                    v
                },
            )
        }
        Ok(output) => (
            false,
            format!(
                "{} returned exit code {:?}",
                engine.as_str(),
                output.status.code()
            ),
        ),
        Err(err) => (false, format!("{} not available: {}", engine.as_str(), err)),
    }
}

fn check_git_repo(cwd: &Path) -> (bool, String) {
    match Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(cwd)
        .output()
    {
        Ok(output) if output.status.success() => {
            let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if s == "true" {
                (true, "inside git work tree".to_string())
            } else {
                (false, format!("unexpected git response: {}", s))
            }
        }
        Ok(output) => (
            false,
            format!("git returned exit code {:?}", output.status.code()),
        ),
        Err(err) => (false, format!("git not available: {}", err)),
    }
}

fn check_runtime_writable(cwd: &Path) -> (bool, String) {
    let runtime_dir = cwd.join(".forge");
    if let Err(err) = fs::create_dir_all(&runtime_dir) {
        return (false, format!("cannot create .forge: {}", err));
    }
    let probe = runtime_dir.join(".doctor_write_probe");
    match fs::write(&probe, "test") {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            (true, ".forge is writable".to_string())
        }
        Err(err) => (false, format!("cannot write to .forge: {}", err)),
    }
}

fn check_config_loadable(cwd: &Path) -> (bool, String) {
    match load_run_config(cwd, &CliOverrides::default()) {
        Ok(_) => (true, "config loaded successfully".to_string()),
        Err(err) => (false, format!("config error: {}", err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn collect_doctor_checks_returns_four_checks() {
        let dir = tempdir().expect("tempdir");
        let checks = collect_doctor_checks(dir.path());
        assert_eq!(checks.len(), 4);
        assert!(checks.iter().any(|c| c.name == "engine_available"));
        assert!(checks.iter().any(|c| c.name == "git_repository"));
        assert!(checks.iter().any(|c| c.name == "runtime_writable"));
        assert!(checks.iter().any(|c| c.name == "config_loadable"));
    }

    #[test]
    fn collect_doctor_warnings_empty_when_files_exist() {
        let dir = tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join(".forge/analyze")).expect("create analyze dir");
        fs::write(dir.path().join(".forgerc"), "engine = \"codex\"\n").expect("write forgerc");
        fs::write(dir.path().join(".forge/analyze/latest.json"), "{}").expect("write latest");

        let warnings = collect_doctor_warnings(dir.path());
        assert!(warnings.is_empty());
    }

    #[test]
    fn collect_doctor_warnings_when_missing_forgerc() {
        let dir = tempdir().expect("tempdir");
        let warnings = collect_doctor_warnings(dir.path());
        assert!(warnings.iter().any(|w| w.contains(".forgerc")));
    }

    #[test]
    fn apply_doctor_fixes_creates_forge_dir() {
        let dir = tempdir().expect("tempdir");
        let fixes = apply_doctor_fixes(dir.path()).expect("fixes");

        assert!(dir.path().join(".forge").exists());
        assert!(fixes.iter().any(|f| f.contains(".forge")));
    }

    #[test]
    fn apply_doctor_fixes_creates_forgerc() {
        let dir = tempdir().expect("tempdir");
        let fixes = apply_doctor_fixes(dir.path()).expect("fixes");

        assert!(dir.path().join(".forgerc").exists());
        assert!(fixes.iter().any(|f| f.contains(".forgerc")));
    }

    #[test]
    fn apply_doctor_fixes_no_changes_when_all_exist() {
        let dir = tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join(".forge")).expect("create forge dir");
        fs::write(dir.path().join(".forgerc"), "engine = \"codex\"\n").expect("write forgerc");

        let fixes = apply_doctor_fixes(dir.path()).expect("fixes");
        assert!(fixes.is_empty());
    }

    #[test]
    fn run_doctor_returns_report() {
        let dir = tempdir().expect("tempdir");
        let report = run_doctor(dir.path(), false, false).expect("report");

        assert_eq!(report.cwd, dir.path().display().to_string());
        assert_eq!(report.checks.len(), 4);
        assert!(report.attempted_fixes.is_empty());
    }

    #[test]
    fn run_doctor_with_fix_applies_fixes() {
        let dir = tempdir().expect("tempdir");
        let report = run_doctor(dir.path(), true, false).expect("report");

        assert!(!report.attempted_fixes.is_empty());
    }
}
