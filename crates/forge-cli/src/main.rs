use anyhow::Result;
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
}

#[derive(Debug, clap::Args)]
struct RunCommand {
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
        None => assistant_mode(cwd),
    }
}

fn assistant_mode(cwd: PathBuf) -> Result<()> {
    println!("forge assistant mode");
    println!("answer the SDD questions. forge will generate specs and run the loop.\n");

    let answers = collect_sdd_answers()?;
    write_sdd_outputs(&cwd, &answers)?;

    println!("\nGenerated:");
    println!("- .forge/plan.md");
    println!("- docs/specs/session/spec.md");
    println!("- docs/specs/session/acceptance.md");
    println!("- docs/specs/session/scenarios.md");
    println!("\nstarting loop...\n");

    run_command(
        RunCommand {
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

fn write_sdd_outputs(cwd: &Path, answers: &SddInterview) -> Result<()> {
    let forge_dir = cwd.join(".forge");
    let docs_dir = cwd.join("docs/specs/session");
    fs::create_dir_all(&forge_dir)?;
    fs::create_dir_all(&docs_dir)?;

    let spec = render_spec(answers);
    let acceptance = render_acceptance(answers);
    let scenarios = render_scenarios(answers);
    let plan = render_plan(answers);

    fs::write(docs_dir.join("spec.md"), spec)?;
    fs::write(docs_dir.join("acceptance.md"), acceptance)?;
    fs::write(docs_dir.join("scenarios.md"), scenarios)?;
    fs::write(forge_dir.join("plan.md"), plan)?;

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
        out.push_str("## Scenario 1\nGiven a planned task When forge run executes Then progress is persisted\n");
    }
    out
}

fn render_plan(a: &SddInterview) -> String {
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_secs();

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
