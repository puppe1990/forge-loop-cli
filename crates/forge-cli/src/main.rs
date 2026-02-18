use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use forge_config::{load_run_config, CliOverrides};
use forge_core::{read_status, run_loop, ExitReason, RunRequest};
use forge_monitor::run_monitor;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    Status(StatusCommand),
    Monitor(MonitorCommand),
    Sdd(SddCommand),
}

#[derive(Debug, clap::Args)]
struct RunCommand {
    #[arg(long = "codex-arg")]
    codex_pre_args: Vec<String>,

    #[arg(long)]
    resume: Option<String>,

    #[arg(long, conflicts_with = "resume")]
    resume_last: bool,

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
struct MonitorCommand {
    #[arg(long, default_value_t = 500)]
    refresh_ms: u64,
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
            resume: None,
            resume_last: false,
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
    let cfg = load_run_config(
        &cwd,
        &CliOverrides {
            codex_pre_args: if cmd.codex_pre_args.is_empty() {
                None
            } else {
                Some(cmd.codex_pre_args)
            },
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

fn status_command(cmd: StatusCommand, cwd: PathBuf) -> Result<()> {
    let cfg = load_run_config(&cwd, &CliOverrides::default())?;
    let runtime_dir = cwd.join(cfg.runtime_dir);
    let status = read_status(&runtime_dir)?;

    if cmd.json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!("state: {}", status.state);
        println!("current_loop: {}", status.current_loop);
        println!("total_loops_executed: {}", status.total_loops_executed);
        println!("completion_indicators: {}", status.completion_indicators);
        println!("exit_signal_seen: {}", status.exit_signal_seen);
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
