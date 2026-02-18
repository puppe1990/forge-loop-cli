use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use forge_config::{load_run_config, CliOverrides, ThinkingMode};
use forge_core::{read_status, run_loop, ExitReason, RunRequest};
use forge_monitor::run_monitor;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Parser)]
#[command(
    name = "forge",
    about = "Spec-driven autonomous loop CLI",
    version,
    after_help = "Without subcommands, forge starts interactive assistant mode."
)]
struct Cli {
    #[arg(long, global = true)]
    cwd: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Run(RunCommand),
    Analyze(AnalyzeCommand),
    Doctor(DoctorCommand),
    Status(StatusCommand),
    Monitor(MonitorCommand),
    Sdd(SddCommand),
}

#[derive(Debug, clap::Args)]
struct RunCommand {
    #[arg(long = "codex-arg")]
    codex_pre_args: Vec<String>,

    #[arg(long, value_enum)]
    thinking: Option<ThinkingArg>,

    #[arg(long)]
    resume: Option<String>,

    #[arg(long, conflicts_with = "resume")]
    resume_last: bool,

    #[arg(long, conflicts_with_all = ["resume", "resume_last"])]
    fresh: bool,

    #[arg(long)]
    max_calls_per_hour: Option<u32>,

    #[arg(long)]
    timeout_minutes: Option<u64>,

    #[arg(long)]
    json: bool,

    #[arg(long, default_value_t = 100)]
    max_loops: u64,
}

#[derive(Debug, clap::Args)]
struct StatusCommand {
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct AnalyzeCommand {
    #[arg(long = "codex-arg")]
    codex_pre_args: Vec<String>,

    #[arg(long, value_enum)]
    thinking: Option<ThinkingArg>,

    #[arg(long, default_value_t = true)]
    modified_only: bool,

    #[arg(long, default_value_t = 25)]
    chunk_size: usize,

    #[arg(long)]
    resume_latest_report: bool,

    #[arg(long)]
    timeout_minutes: Option<u64>,

    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct MonitorCommand {
    #[arg(long, default_value_t = 500)]
    refresh_ms: u64,
}

#[derive(Debug, clap::Args)]
struct DoctorCommand {
    #[arg(long)]
    json: bool,

    #[arg(long)]
    fix: bool,

    #[arg(long)]
    strict: bool,
}

#[derive(Debug, clap::Args)]
struct SddCommand {
    #[command(subcommand)]
    action: SddAction,
}

#[derive(Debug, Subcommand)]
enum SddAction {
    List(SddListCommand),
    Load(SddLoadCommand),
}

#[derive(Debug, clap::Args)]
struct SddListCommand {
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct SddLoadCommand {
    id: String,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum ThinkingArg {
    Off,
    Summary,
    Raw,
}

impl From<ThinkingArg> for ThinkingMode {
    fn from(value: ThinkingArg) -> Self {
        match value {
            ThinkingArg::Off => ThinkingMode::Off,
            ThinkingArg::Summary => ThinkingMode::Summary,
            ThinkingArg::Raw => ThinkingMode::Raw,
        }
    }
}

#[derive(Debug)]
struct SddInterview {
    project_name: String,
    product_goal: String,
    target_users: String,
    in_scope: String,
    out_of_scope: String,
    constraints: String,
    acceptance_criteria: String,
    scenarios: String,
    tests: String,
    max_loops: u64,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cwd = resolve_cwd(cli.cwd)?;

    match cli.command {
        Some(Commands::Run(cmd)) => run_command(cmd, cwd),
        Some(Commands::Analyze(cmd)) => analyze_command(cmd, cwd),
        Some(Commands::Doctor(cmd)) => doctor_command(cmd, cwd),
        Some(Commands::Status(cmd)) => status_command(cmd, cwd),
        Some(Commands::Monitor(cmd)) => monitor_command(cmd, cwd),
        Some(Commands::Sdd(cmd)) => sdd_command(cmd, cwd),
        None => assistant_mode(cwd),
    }
}

fn assistant_mode(cwd: PathBuf) -> Result<()> {
    println!("forge assistant mode");
    println!("answer the SDD questions. forge will generate specs and run the loop.\n");

    let answers = collect_sdd_answers()?;
    let sdd_id = create_sdd_snapshot(&cwd, &answers)?;
    activate_sdd(&cwd, &sdd_id)?;

    println!("\nGenerated and activated SDD: {sdd_id}");
    println!("- .forge/sdds/{sdd_id}/plan.md");
    println!("- .forge/plan.md");
    println!("- docs/specs/session/spec.md");
    println!("- docs/specs/session/acceptance.md");
    println!("- docs/specs/session/scenarios.md");
    println!("\nUse `forge sdd list` and `forge sdd load <id>` to switch plans.");
    println!("\nstarting loop...\n");

    run_command(
        RunCommand {
            codex_pre_args: Vec::new(),
            thinking: None,
            resume: None,
            resume_last: false,
            fresh: false,
            max_calls_per_hour: None,
            timeout_minutes: None,
            json: false,
            max_loops: answers.max_loops,
        },
        cwd,
    )
}

fn sdd_command(cmd: SddCommand, cwd: PathBuf) -> Result<()> {
    match cmd.action {
        SddAction::List(list) => sdd_list(cwd.as_path(), list.json),
        SddAction::Load(load) => {
            activate_sdd(cwd.as_path(), &load.id)?;
            println!("loaded sdd: {}", load.id);
            Ok(())
        }
    }
}

fn sdd_list(cwd: &Path, as_json: bool) -> Result<()> {
    let root = sdd_root(cwd);
    let current = current_sdd_id(cwd)?;

    if !root.exists() {
        if as_json {
            println!("[]");
        } else {
            println!("no sdds found");
        }
        return Ok(());
    }

    let mut entries = fs::read_dir(&root)
        .with_context(|| format!("failed to read {}", root.display()))?
        .flatten()
        .filter(|e| e.path().is_dir())
        .collect::<Vec<_>>();

    entries.sort_by_key(|e| e.file_name());
    entries.reverse();

    if as_json {
        let list = entries
            .iter()
            .map(|e| {
                let id = e.file_name().to_string_lossy().to_string();
                let meta = read_sdd_meta(cwd, &id).unwrap_or_default();
                serde_json::json!({
                    "id": id,
                    "project_name": meta.project_name,
                    "goal": meta.goal,
                    "created_at_epoch": meta.created_at_epoch,
                    "current": current.as_deref() == Some(e.file_name().to_string_lossy().as_ref())
                })
            })
            .collect::<Vec<_>>();
        println!("{}", serde_json::to_string_pretty(&list)?);
        return Ok(());
    }

    if entries.is_empty() {
        println!("no sdds found");
        return Ok(());
    }

    println!("available sdds:");
    for entry in entries {
        let id = entry.file_name().to_string_lossy().to_string();
        let meta = read_sdd_meta(cwd, &id).unwrap_or_default();
        let marker = if current.as_deref() == Some(id.as_str()) {
            "*"
        } else {
            " "
        };
        let title = if meta.project_name.is_empty() {
            "(no project name)".to_string()
        } else {
            meta.project_name
        };
        println!("{} {} - {}", marker, id, title);
    }
    println!("\n* current");
    Ok(())
}

fn collect_sdd_answers() -> Result<SddInterview> {
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

    let max_loops_str = ask("max loops for execution", "100")?;
    let max_loops = max_loops_str.trim().parse::<u64>().unwrap_or(100);

    Ok(SddInterview {
        project_name,
        product_goal,
        target_users,
        in_scope,
        out_of_scope,
        constraints,
        acceptance_criteria,
        scenarios,
        tests,
        max_loops,
    })
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

fn create_sdd_snapshot(cwd: &Path, answers: &SddInterview) -> Result<String> {
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

    let meta = serde_json::json!({
        "id": id,
        "project_name": answers.project_name,
        "goal": answers.product_goal,
        "created_at_epoch": epoch_now(),
    });
    fs::write(
        snapshot_dir.join("meta.json"),
        serde_json::to_string_pretty(&meta)?,
    )?;

    Ok(id)
}

fn activate_sdd(cwd: &Path, id: &str) -> Result<()> {
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

fn copy_required(from: PathBuf, to: PathBuf) -> Result<()> {
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
        "# Execution Plan\n\nGenerated at epoch {}\n\n## Goal\n{}\n\n## Scope\n- In: {}\n- Out: {}\n\n## Constraints\n{}\n\n## Acceptance\n{}\n\n## Scenarios\n{}\n\n## Test Strategy\n{}\n\nExecute this plan incrementally. Only stop when completion indicators are present and EXIT_SIGNAL is true. Persist status and progress in .forge/.\n",
        epoch,
        a.product_goal,
        a.in_scope,
        a.out_of_scope,
        a.constraints,
        a.acceptance_criteria,
        a.scenarios,
        a.tests,
    )
}

fn run_command(cmd: RunCommand, cwd: PathBuf) -> Result<()> {
    if cmd.fresh {
        cleanup_runtime_state(&cwd)?;
    }

    let codex_pre_args = cmd.codex_pre_args;
    let codex_exec_args = if cmd.fresh {
        Some(vec!["--ephemeral".to_string()])
    } else {
        None
    };

    let cfg = load_run_config(
        &cwd,
        &CliOverrides {
            codex_pre_args: if codex_pre_args.is_empty() {
                None
            } else {
                Some(codex_pre_args)
            },
            codex_exec_args,
            thinking_mode: cmd.thinking.map(Into::into),
            max_calls_per_hour: cmd.max_calls_per_hour,
            timeout_minutes: cmd.timeout_minutes,
            resume: cmd.resume,
            resume_last: cmd.resume_last,
        },
    )?;

    let outcome = run_loop(RunRequest {
        cwd,
        config: cfg,
        max_loops: cmd.max_loops,
    })?;

    if cmd.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "reason": format!("{:?}", outcome.reason),
                "loops_executed": outcome.loops_executed,
                "status": outcome.status,
            }))?
        );
    } else {
        println!(
            "state={} reason={:?} loops={}",
            outcome.status.state, outcome.reason, outcome.loops_executed
        );
    }

    std::process::exit(match outcome.reason {
        ExitReason::Completed => 0,
        ExitReason::CircuitOpened => 2,
        ExitReason::RateLimited => 3,
        ExitReason::MaxLoopsReached => 4,
    });
}

fn analyze_command(cmd: AnalyzeCommand, cwd: PathBuf) -> Result<()> {
    let codex_pre_args_override = if cmd.codex_pre_args.is_empty() {
        None
    } else {
        Some(cmd.codex_pre_args.clone())
    };

    let cfg = load_run_config(
        &cwd,
        &CliOverrides {
            codex_pre_args: codex_pre_args_override,
            codex_exec_args: Some(vec!["--ephemeral".to_string()]),
            thinking_mode: cmd.thinking.map(Into::into),
            max_calls_per_hour: None,
            timeout_minutes: cmd.timeout_minutes,
            resume: None,
            resume_last: false,
        },
    )?;

    if cmd.resume_latest_report {
        return analyze_resume_latest(cmd, cwd, cfg);
    }

    let files = if cmd.modified_only {
        list_modified_files(&cwd)?
    } else {
        Vec::new()
    };
    if cmd.modified_only && files.is_empty() {
        if cmd.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "modified_files": 0,
                    "report": "No modified files found.",
                    "exit_signal": true
                }))?
            );
        } else {
            println!("No modified files found.\nEXIT_SIGNAL: true");
        }
        return Ok(());
    }

    let chunk_size = cmd.chunk_size.max(1);
    let chunks = files
        .chunks(chunk_size)
        .map(|slice| slice.to_vec())
        .collect::<Vec<_>>();

    let mut chunk_reports = Vec::new();
    let mut timed_out_chunks = 0_u64;
    let mut failed_chunks = 0_u64;
    for (idx, chunk) in chunks.iter().enumerate() {
        eprintln!(
            "analyze: chunk {}/{} ({} files) started",
            idx + 1,
            chunks.len(),
            chunk.len()
        );
        let prompt = build_analyze_prompt(chunk, &format!("chunk {}/{}", idx + 1, chunks.len()));
        let run = run_codex_exec_with_timeout(
            &cfg.codex_cmd,
            &cfg.codex_pre_args,
            &cfg.codex_exec_args,
            &cwd,
            &prompt,
            cfg.timeout_minutes,
        )?;
        if run.timed_out {
            timed_out_chunks += 1;
        }
        if run.exit_code != Some(0) {
            failed_chunks += 1;
        }
        eprintln!(
            "analyze: chunk {}/{} done (exit_code={:?}, timed_out={})",
            idx + 1,
            chunks.len(),
            run.exit_code,
            run.timed_out
        );
        let label = format!(
            "## Chunk {}/{} ({} files)",
            idx + 1,
            chunks.len(),
            chunk.len()
        );
        chunk_reports.push(format!("{label}\n{}", run.report));
    }

    let report = if chunk_reports.len() <= 1 {
        chunk_reports
            .first()
            .cloned()
            .unwrap_or_else(|| "No analysis output.".to_string())
    } else {
        let joined = chunk_reports.join("\n\n");
        eprintln!("analyze: synthesis started");
        let synthesis_prompt = format!(
            "Consolidate the following chunk analyses into exactly:\n1) Critical risks\n2) High risks\n3) Medium risks\n4) Suggested next actions\nEnd with: EXIT_SIGNAL: true\n\n{joined}"
        );
        let synthesis = run_codex_exec_with_timeout(
            &cfg.codex_cmd,
            &cfg.codex_pre_args,
            &cfg.codex_exec_args,
            &cwd,
            &synthesis_prompt,
            cfg.timeout_minutes,
        )?;
        eprintln!(
            "analyze: synthesis done (exit_code={:?}, timed_out={})",
            synthesis.exit_code, synthesis.timed_out
        );
        if synthesis.timed_out || synthesis.exit_code != Some(0) {
            format!(
                "Consolidation fallback (timed_out={}, exit_code={:?}).\n\n{}",
                synthesis.timed_out, synthesis.exit_code, joined
            )
        } else {
            synthesis.report
        }
    };

    let persisted = persist_analyze_report(
        &cwd,
        AnalyzePersistInput {
            files: &files,
            chunks: chunks.len(),
            chunk_size,
            timed_out_chunks,
            failed_chunks,
            chunk_reports: &chunk_reports,
            report: &report,
        },
    )?;

    if cmd.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "modified_files": files.len(),
                "chunks": chunks.len(),
                "chunk_size": chunk_size,
                "timed_out_chunks": timed_out_chunks,
                "failed_chunks": failed_chunks,
                "latest_report_path": persisted.latest_path,
                "history_report_path": persisted.history_path,
                "report": report,
            }))?
        );
    } else {
        println!("latest_report_path: {}", persisted.latest_path);
        println!("history_report_path: {}", persisted.history_path);
        println!("{}", report);
    }

    if timed_out_chunks > 0 {
        bail!("analyze timed out in {} chunk(s)", timed_out_chunks);
    }
    if failed_chunks > 0 {
        bail!("analyze failed in {} chunk(s)", failed_chunks);
    }
    Ok(())
}

fn analyze_resume_latest(
    cmd: AnalyzeCommand,
    cwd: PathBuf,
    cfg: forge_config::RunConfig,
) -> Result<()> {
    let latest = load_latest_analyze_payload(&cwd)?;
    let files = latest
        .get("files")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToString::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let chunk_reports = latest
        .get("chunk_reports")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToString::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if chunk_reports.is_empty() {
        bail!("no chunk_reports found in .forge/analyze/latest.json");
    }

    eprintln!(
        "analyze: resume from latest report ({} chunk reports)",
        chunk_reports.len()
    );

    let joined = chunk_reports.join("\n\n");
    let synthesis_prompt = format!(
        "Consolidate the following chunk analyses into exactly:\n1) Critical risks\n2) High risks\n3) Medium risks\n4) Suggested next actions\nEnd with: EXIT_SIGNAL: true\n\n{joined}"
    );
    let synthesis = run_codex_exec_with_timeout(
        &cfg.codex_cmd,
        &cfg.codex_pre_args,
        &cfg.codex_exec_args,
        &cwd,
        &synthesis_prompt,
        cfg.timeout_minutes,
    )?;

    let report = if synthesis.timed_out || synthesis.exit_code != Some(0) {
        format!(
            "Consolidation fallback (timed_out={}, exit_code={:?}).\n\n{}",
            synthesis.timed_out, synthesis.exit_code, joined
        )
    } else {
        synthesis.report
    };

    let persisted = persist_analyze_report(
        &cwd,
        AnalyzePersistInput {
            files: &files,
            chunks: chunk_reports.len(),
            chunk_size: latest
                .get("chunk_size")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize,
            timed_out_chunks: if synthesis.timed_out { 1 } else { 0 },
            failed_chunks: if synthesis.exit_code == Some(0) { 0 } else { 1 },
            chunk_reports: &chunk_reports,
            report: &report,
        },
    )?;

    if cmd.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "mode": "resume_latest_report",
                "chunk_reports": chunk_reports.len(),
                "latest_report_path": persisted.latest_path,
                "history_report_path": persisted.history_path,
                "report": report,
                "timed_out": synthesis.timed_out,
                "exit_code": synthesis.exit_code,
            }))?
        );
    } else {
        println!("latest_report_path: {}", persisted.latest_path);
        println!("history_report_path: {}", persisted.history_path);
        println!("{}", report);
    }

    if synthesis.timed_out {
        bail!("resume synthesis timed out");
    }
    if synthesis.exit_code != Some(0) {
        bail!(
            "resume synthesis failed with exit code {:?}",
            synthesis.exit_code
        );
    }
    Ok(())
}

fn list_modified_files(cwd: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["diff", "--name-only"])
        .current_dir(cwd)
        .output()
        .context("failed to list modified files with git")?;
    if !output.status.success() {
        bail!(
            "git diff failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let files = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    Ok(files)
}

fn build_analyze_prompt(files: &[String], scope_label: &str) -> String {
    let mut out = String::from(
        "Analyze ONLY these modified files and report exactly:\n1) Critical risks\n2) High risks\n3) Medium risks\n4) Suggested next actions\nDo not propose edits, only analysis.\nEnd with: EXIT_SIGNAL: true\n\nScope: ",
    );
    out.push_str(scope_label);
    out.push_str("\n\nModified files:\n");
    for file in files {
        out.push_str("- ");
        out.push_str(file);
        out.push('\n');
    }
    out
}

fn load_latest_analyze_payload(cwd: &Path) -> Result<serde_json::Value> {
    let path = cwd.join(".forge").join("analyze").join("latest.json");
    if !path.exists() {
        bail!("latest analyze report not found at {}", path.display());
    }
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("invalid json in {}", path.display()))?;
    Ok(value)
}

#[derive(Debug)]
struct CodexExecRun {
    report: String,
    exit_code: Option<i32>,
    timed_out: bool,
}

#[derive(Debug)]
struct AnalyzePersistPaths {
    latest_path: String,
    history_path: String,
}

struct AnalyzePersistInput<'a> {
    files: &'a [String],
    chunks: usize,
    chunk_size: usize,
    timed_out_chunks: u64,
    failed_chunks: u64,
    chunk_reports: &'a [String],
    report: &'a str,
}

fn persist_analyze_report(
    cwd: &Path,
    input: AnalyzePersistInput<'_>,
) -> Result<AnalyzePersistPaths> {
    let analyze_dir = cwd.join(".forge").join("analyze");
    let history_dir = analyze_dir.join("history");
    fs::create_dir_all(&history_dir)
        .with_context(|| format!("failed to create {}", history_dir.display()))?;

    let now = epoch_now();
    let payload = serde_json::json!({
        "created_at_epoch": now,
        "modified_files": input.files.len(),
        "chunks": input.chunks,
        "chunk_size": input.chunk_size,
        "timed_out_chunks": input.timed_out_chunks,
        "failed_chunks": input.failed_chunks,
        "files": input.files,
        "chunk_reports": input.chunk_reports,
        "report": input.report,
    });

    let latest_path = analyze_dir.join("latest.json");
    fs::write(&latest_path, serde_json::to_string_pretty(&payload)?)
        .with_context(|| format!("failed to write {}", latest_path.display()))?;

    let history_path = history_dir.join(format!("{}.json", now));
    fs::write(&history_path, serde_json::to_string_pretty(&payload)?)
        .with_context(|| format!("failed to write {}", history_path.display()))?;

    Ok(AnalyzePersistPaths {
        latest_path: latest_path.display().to_string(),
        history_path: history_path.display().to_string(),
    })
}

fn run_codex_exec_with_timeout(
    codex_cmd: &str,
    codex_pre_args: &[String],
    codex_exec_args: &[String],
    cwd: &Path,
    prompt: &str,
    timeout_minutes: u64,
) -> Result<CodexExecRun> {
    let mut args = codex_pre_args.to_vec();
    args.push("exec".to_string());
    args.extend(codex_exec_args.iter().cloned());
    args.push("--json".to_string());
    args.push(prompt.to_string());

    let timeout = Duration::from_secs(timeout_minutes.saturating_mul(60));
    let mut child = Command::new(codex_cmd)
        .args(&args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to execute {}", codex_cmd))?;
    let started = Instant::now();
    let mut timed_out = false;
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            let _ = child.kill();
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }
    let output = child
        .wait_with_output()
        .with_context(|| format!("failed waiting for {}", codex_cmd))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let report = extract_last_agent_message(&stdout).unwrap_or_else(|| {
        let merged = format!("{} {}", stdout.trim(), stderr.trim());
        merged.chars().take(4000).collect()
    });

    Ok(CodexExecRun {
        report,
        exit_code: output.status.code(),
        timed_out,
    })
}

fn extract_last_agent_message(stdout: &str) -> Option<String> {
    let mut last = None;
    for line in stdout.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|v| v.as_str()) != Some("item.completed") {
            continue;
        }
        let Some(item) = value.get("item") else {
            continue;
        };
        if item.get("type").and_then(|v| v.as_str()) != Some("agent_message") {
            continue;
        }
        if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
            last = Some(text.to_string());
        }
    }
    last
}

fn cleanup_runtime_state(cwd: &Path) -> Result<()> {
    let runtime_dir = cwd.join(".forge");
    fs::create_dir_all(&runtime_dir)
        .with_context(|| format!("failed to create {}", runtime_dir.display()))?;

    let files = [
        "status.json",
        "progress.json",
        "live.log",
        ".session_id",
        ".call_count",
        ".last_reset",
        ".circuit_breaker_state",
        ".circuit_breaker_history",
    ];

    for file in files {
        let path = runtime_dir.join(file);
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
        }
    }

    Ok(())
}

fn status_command(cmd: StatusCommand, cwd: PathBuf) -> Result<()> {
    let cfg = load_run_config(&cwd, &CliOverrides::default())?;
    let runtime_dir = cwd.join(cfg.runtime_dir);
    let status = read_status(&runtime_dir)?;

    if cmd.json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        let loop_timer = if status.current_loop_started_at_epoch == 0 {
            "-".to_string()
        } else {
            format_duration(epoch_now().saturating_sub(status.current_loop_started_at_epoch))
        };

        println!("state: {}", status.state);
        println!("thinking_mode: {}", status.thinking_mode);
        println!("current_loop: {}", status.current_loop);
        println!("loop_timer: {}", loop_timer);
        println!("total_loops_executed: {}", status.total_loops_executed);
        println!("completion_indicators: {}", status.completion_indicators);
        println!("exit_signal_seen: {}", status.exit_signal_seen);
        println!(
            "last_heartbeat_at_epoch: {}",
            status.last_heartbeat_at_epoch
        );
        println!(
            "session_id: {}",
            status.session_id.unwrap_or_else(|| "-".to_string())
        );
        println!("updated_at_epoch: {}", status.updated_at_epoch);
    }
    Ok(())
}

fn monitor_command(cmd: MonitorCommand, cwd: PathBuf) -> Result<()> {
    let cfg = load_run_config(&cwd, &CliOverrides::default())?;
    let runtime_dir: PathBuf = cwd.join(cfg.runtime_dir);
    run_monitor(&runtime_dir, cmd.refresh_ms)
}

fn doctor_command(cmd: DoctorCommand, cwd: PathBuf) -> Result<()> {
    let before = collect_doctor_checks(&cwd);
    let before_warnings = collect_doctor_warnings(&cwd);
    let mut attempted_fixes = Vec::new();
    if cmd.fix {
        attempted_fixes = apply_doctor_fixes(&cwd)?;
    }
    let checks = collect_doctor_checks(&cwd);
    let failed = checks.iter().filter(|c| !c.ok).count();
    let warnings = collect_doctor_warnings(&cwd);
    let strict_failed = cmd.strict && !warnings.is_empty();

    if cmd.json {
        let report = serde_json::json!({
            "cwd": cwd.display().to_string(),
            "ok": failed == 0 && !strict_failed,
            "failed_checks": failed,
            "fix_mode": cmd.fix,
            "strict_mode": cmd.strict,
            "attempted_fixes": attempted_fixes,
            "before_warnings": before_warnings,
            "before": before
                .iter()
                .map(|c| serde_json::json!({
                    "name": c.name,
                    "ok": c.ok,
                    "detail": c.detail,
                }))
                .collect::<Vec<_>>(),
            "warnings": warnings,
            "checks": checks
                .iter()
                .map(|c| serde_json::json!({
                    "name": c.name,
                    "ok": c.ok,
                    "detail": c.detail,
                }))
                .collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("forge doctor");
        println!("cwd: {}", cwd.display());
        println!("strict_mode: {}", cmd.strict);
        if cmd.fix {
            if attempted_fixes.is_empty() {
                println!("- fix: no changes applied");
            } else {
                for fix in &attempted_fixes {
                    println!("- fix: {}", fix);
                }
            }
        }
        for c in &checks {
            let status = if c.ok { "ok" } else { "fail" };
            println!("- {}: {} ({})", c.name, status, c.detail);
        }
        if warnings.is_empty() {
            println!("- warnings: none");
        } else {
            for warning in &warnings {
                println!("- warning: {}", warning);
            }
        }
    }

    if failed > 0 {
        bail!("doctor found {} failing check(s)", failed);
    }
    if strict_failed {
        bail!("doctor strict mode failed: {} warning(s)", warnings.len());
    }
    Ok(())
}

#[derive(Debug)]
struct DoctorCheck {
    name: &'static str,
    ok: bool,
    detail: String,
}

fn collect_doctor_checks(cwd: &Path) -> Vec<DoctorCheck> {
    let codex = check_codex_available();
    let git = check_git_repo(cwd);
    let write = check_runtime_writable(cwd);
    let config = check_config_loadable(cwd);
    vec![
        DoctorCheck {
            name: "codex_available",
            ok: codex.0,
            detail: codex.1,
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

fn apply_doctor_fixes(cwd: &Path) -> Result<Vec<String>> {
    let mut fixes = Vec::new();

    let runtime_dir = cwd.join(".forge");
    if !runtime_dir.exists() {
        fs::create_dir_all(&runtime_dir)
            .with_context(|| format!("failed to create {}", runtime_dir.display()))?;
        fixes.push("created .forge runtime directory".to_string());
    }

    let forgerc = cwd.join(".forgerc");
    if !forgerc.exists() {
        let template = "# forge defaults\nmax_calls_per_hour = 100\ntimeout_minutes = 15\n";
        fs::write(&forgerc, template)
            .with_context(|| format!("failed to write {}", forgerc.display()))?;
        fixes.push("created .forgerc with baseline defaults".to_string());
    }

    Ok(fixes)
}

fn collect_doctor_warnings(cwd: &Path) -> Vec<String> {
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

fn check_codex_available() -> (bool, String) {
    match Command::new("codex").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let v = String::from_utf8_lossy(&output.stdout).trim().to_string();
            (
                true,
                if v.is_empty() {
                    "codex found".to_string()
                } else {
                    v
                },
            )
        }
        Ok(output) => (
            false,
            format!("codex returned exit code {:?}", output.status.code()),
        ),
        Err(err) => (false, format!("codex not available: {}", err)),
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
    match fs::write(&probe, "ok") {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            (true, "runtime is writable".to_string())
        }
        Err(err) => (false, format!("cannot write runtime probe: {}", err)),
    }
}

fn check_config_loadable(cwd: &Path) -> (bool, String) {
    match load_run_config(cwd, &CliOverrides::default()) {
        Ok(_) => (true, ".forgerc/env/defaults load".to_string()),
        Err(err) => (false, format!("config error: {}", err)),
    }
}

fn resolve_cwd(cwd: Option<PathBuf>) -> Result<PathBuf> {
    let path = match cwd {
        Some(p) => p,
        None => env::current_dir()?,
    };
    Ok(path.canonicalize()?)
}

fn sdd_root(cwd: &Path) -> PathBuf {
    cwd.join(".forge").join("sdds")
}

fn current_sdd_id(cwd: &Path) -> Result<Option<String>> {
    let path = cwd.join(".forge").join("current_sdd");
    if !path.exists() {
        return Ok(None);
    }
    let id = fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?
        .trim()
        .to_string();
    if id.is_empty() {
        Ok(None)
    } else {
        Ok(Some(id))
    }
}

#[derive(Default)]
struct SddMeta {
    project_name: String,
    goal: String,
    created_at_epoch: u64,
}

fn read_sdd_meta(cwd: &Path, id: &str) -> Result<SddMeta> {
    let path = sdd_root(cwd).join(id).join("meta.json");
    if !path.exists() {
        return Ok(SddMeta::default());
    }
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(SddMeta {
        project_name: value
            .get("project_name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        goal: value
            .get("goal")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        created_at_epoch: value
            .get("created_at_epoch")
            .and_then(|v| v.as_u64())
            .unwrap_or_default(),
    })
}

fn slugify(input: &str, fallback: &str) -> String {
    let slug = input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();

    let slug = slug
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    if slug.is_empty() {
        fallback.to_string()
    } else {
        slug
    }
}

fn epoch_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_secs()
}

fn format_duration(total_secs: u64) -> String {
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}
