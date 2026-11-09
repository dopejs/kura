//! `kura-cua-eval`: run the desktop task suite against a daemon and a
//! paired `kura-device` (docs/harness/remote-computer-use.md §12 slice 5).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use kura_cua_eval::client::Client;
use kura_cua_eval::desktop::MacDesktop;
use kura_cua_eval::report;
use kura_cua_eval::runner::{ApprovalMode, RunConfig, Runner, compose_query};
use kura_cua_eval::suite::{Placeholders, Suite};

#[derive(Parser)]
#[command(
    name = "kura-cua-eval",
    version,
    about = "Desktop task evaluation for remote computer use"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Checks a suite file and lists its tasks.
    Validate { suite: PathBuf },
    /// Prints what a run would ask and do, touching nothing.
    Plan {
        suite: PathBuf,
        /// Only these task ids (comma separated).
        #[arg(long, value_delimiter = ',')]
        only: Vec<String>,
    },
    /// Pairs with a daemon you run yourself (local pairing) and saves the
    /// API token to a file readable only by you.
    Login {
        #[arg(long)]
        server: String,
        #[arg(long)]
        token_file: PathBuf,
    },
    /// Prints a single-use code for `kura-device pair <code> --server …`.
    DeviceCode {
        #[arg(long)]
        server: String,
        #[arg(long)]
        token_file: PathBuf,
    },
    /// Runs the suite and writes report.json and summary.md.
    Run(RunArgs),
}

#[derive(clap::Args)]
struct RunArgs {
    suite: PathBuf,
    #[arg(long)]
    server: String,
    #[arg(long)]
    token_file: PathBuf,
    /// Where report.json and summary.md go (created).
    #[arg(long)]
    out: PathBuf,
    /// Provider id for the chat turns; the daemon's default when absent.
    #[arg(long, default_value = "")]
    provider: String,
    #[arg(long, default_value = "")]
    model: String,
    /// Each task this many times (the verdict wants the suite's
    /// minRunsPerTask).
    #[arg(long, default_value_t = 3)]
    repeats: u32,
    #[arg(long, value_delimiter = ',')]
    only: Vec<String>,
    #[arg(long, value_enum, default_value_t = Approvals::Reject)]
    approvals: Approvals,
    /// Parent of the run's work directories; the system temp dir when
    /// absent. Nothing outside `<work-root>/<run-id>` is written.
    #[arg(long)]
    work_root: Option<PathBuf>,
}

#[derive(Clone, Copy, ValueEnum)]
enum Approvals {
    /// Turn down every computer approval the agent raises, at once.
    Reject,
    /// Leave them for a person to answer.
    Manual,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let outcome = match cli.command {
        Command::Validate { suite } => validate(&suite),
        Command::Plan { suite, only } => plan(&suite, &only),
        Command::Login { server, token_file } => login(&server, &token_file).await,
        Command::DeviceCode { server, token_file } => device_code(&server, &token_file).await,
        Command::Run(args) => run(args).await,
    };
    if let Err(err) = outcome {
        eprintln!("kura-cua-eval: {err}");
        std::process::exit(1);
    }
}

fn validate(path: &Path) -> Result<(), String> {
    let suite = Suite::load(path)?;
    println!(
        "{} v{} ({}): {} tasks",
        suite.suite,
        suite.version,
        suite.platform,
        suite.tasks.len()
    );
    for task in &suite.tasks {
        println!(
            "  {:<24} {:<10} reference {:>2}, budget {:>2}, {}s",
            task.id,
            task.category,
            task.reference_steps,
            task.budget(&suite.defaults),
            task.time_limit(&suite.defaults)
        );
    }
    Ok(())
}

fn plan(path: &Path, only: &[String]) -> Result<(), String> {
    let suite = Suite::load(path)?;
    let root = std::env::temp_dir().join("kura-cua-eval").join("<run-id>");
    for task in suite
        .tasks
        .iter()
        .filter(|task| only.is_empty() || only.contains(&task.id))
    {
        let work = root.join(format!("{}-r1", task.id));
        let placeholders = Placeholders { work: &work };
        println!("== {} ({})", task.id, task.category);
        println!(
            "query:\n{}",
            compose_query(&placeholders.apply(&task.instruction), &suite.preamble)
        );
        if !task.requires.apps_not_running.is_empty() {
            println!("requires not running: {:?}", task.requires.apps_not_running);
        }
        println!(
            "setup: {}",
            serde_json::to_string(&task.setup).unwrap_or_default()
        );
        println!(
            "check: {}",
            serde_json::to_string(&task.check).unwrap_or_default()
        );
        println!(
            "teardown: {}",
            serde_json::to_string(&task.teardown).unwrap_or_default()
        );
        println!(
            "budget {} steps (reference {}), {}s\n",
            task.budget(&suite.defaults),
            task.reference_steps,
            task.time_limit(&suite.defaults)
        );
    }
    Ok(())
}

async fn login(server: &str, token_file: &Path) -> Result<(), String> {
    let token = Client::login_local(server, "kura-cua-eval").await?;
    write_private(token_file, &token)?;
    println!("token saved to {}", token_file.display());
    Ok(())
}

async fn device_code(server: &str, token_file: &Path) -> Result<(), String> {
    let client = Client::new(server, &read_token(token_file)?)?;
    let code = client.device_pairing_code().await?;
    println!("{code}");
    println!("on the Mac: kura-device pair {code} --server {server}");
    Ok(())
}

async fn run(args: RunArgs) -> Result<(), String> {
    let suite = Suite::load(&args.suite)?;
    if suite.platform != "macos" {
        return Err(format!(
            "suite platform {:?}: this command drives macOS only",
            suite.platform
        ));
    }
    if !cfg!(target_os = "macos") {
        return Err("a macOS suite runs on the Mac the device is on".into());
    }
    let unknown: Vec<&String> = args
        .only
        .iter()
        .filter(|id| !suite.tasks.iter().any(|task| &task.id == *id))
        .collect();
    if !unknown.is_empty() {
        return Err(format!("unknown task ids: {unknown:?}"));
    }
    let client = Client::new(&args.server, &read_token(&args.token_file)?)?;
    let devices = client.devices().await?;
    if !devices.iter().any(|device| device.online) {
        return Err("no device is online for this token's tenant; start `kura-device run`".into());
    }

    let run_id = format!(
        "cua-{}-{}",
        chrono::Utc::now().format("%Y%m%d-%H%M%S"),
        &uuid::Uuid::now_v7().simple().to_string()[24..]
    );
    let parent = args
        .work_root
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join("kura-cua-eval"));
    std::fs::create_dir_all(&parent).map_err(|err| format!("{}: {err}", parent.display()))?;
    // The real path: TextEdit and Finder report /private/var/…, not /var/….
    let parent =
        std::fs::canonicalize(&parent).map_err(|err| format!("{}: {err}", parent.display()))?;
    let work_root = parent.join(&run_id);
    std::fs::create_dir_all(&work_root).map_err(|err| format!("{}: {err}", work_root.display()))?;
    std::fs::create_dir_all(&args.out).map_err(|err| format!("{}: {err}", args.out.display()))?;

    eprintln!("[cua-eval] run {run_id}: work in {}", work_root.display());
    let started_at = chrono::Utc::now().to_rfc3339();
    let runner = Runner {
        client,
        desktop: Arc::new(MacDesktop),
        config: RunConfig {
            run_id: run_id.clone(),
            work_root,
            provider: args.provider.clone(),
            model: args.model.clone(),
            repeats: args.repeats,
            only: args.only.clone(),
            approvals: match args.approvals {
                Approvals::Reject => ApprovalMode::Reject,
                Approvals::Manual => ApprovalMode::Manual,
            },
            poll: Duration::from_millis(500),
            settle: Duration::from_secs(1),
            cancel_grace: Duration::from_secs(60),
        },
    };
    let result = runner.run(&suite).await;
    let report = report::build(
        &suite,
        report::RunInfo {
            run_id: &run_id,
            provider: &args.provider,
            model: &args.model,
            started_at,
            finished_at: chrono::Utc::now().to_rfc3339(),
        },
        result,
    );
    let json = serde_json::to_string_pretty(&report).map_err(|err| err.to_string())?;
    let json_path = args.out.join("report.json");
    let md_path = args.out.join("summary.md");
    std::fs::write(&json_path, json).map_err(|err| format!("{}: {err}", json_path.display()))?;
    std::fs::write(&md_path, report::markdown(&report))
        .map_err(|err| format!("{}: {err}", md_path.display()))?;
    println!("{}", report::markdown(&report));
    println!("wrote {} and {}", json_path.display(), md_path.display());
    if report.aborted.is_empty() {
        Ok(())
    } else {
        Err(format!("the run stopped early: {}", report.aborted))
    }
}

fn read_token(path: &Path) -> Result<String, String> {
    let token =
        std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(format!("{} is empty", path.display()));
    }
    Ok(token)
}

fn write_private(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write as _;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .map_err(|err| format!("{}: {err}", path.display()))?;
    file.write_all(text.as_bytes())
        .map_err(|err| format!("{}: {err}", path.display()))
}
