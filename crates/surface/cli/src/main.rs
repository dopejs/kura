//! kura — the Kura command line.
//!
//! ```text
//! kura daemon run|start|stop|status   manage the daemon process
//! kura tui [...]                      launch the terminal client
//! kura web [--port] [--no-open]       serve + open the web operator shell
//! kura config show|path|set|edit      inspect and edit configuration
//! ```
//!
//! `daemon run` is the foreground daemon;
//! `daemon start` detaches it with a pidfile + logfile under the data dir
//! and waits for `/healthz`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "kura",
    version,
    about = "Kura — a personal agent OS",
    propagate_version = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage the daemon process
    #[command(subcommand)]
    Daemon(DaemonCommand),
    /// Launch the terminal client (kura-tui)
    #[command(trailing_var_arg = true)]
    Tui {
        /// Arguments passed through to kura-tui
        #[arg(allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Serve and open the web operator shell
    Web {
        /// Port for the local web server
        #[arg(long, default_value_t = 4173)]
        port: u16,
        /// Directory holding the built web assets (defaults: $KURA_WEB_DIST,
        /// <exe dir>/web, <exe dir>/../share/kura/web)
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Do not open the browser automatically
        #[arg(long)]
        no_open: bool,
    },
    /// Inspect and edit configuration
    #[command(subcommand)]
    Config(ConfigCommand),
}

#[derive(Subcommand)]
enum DaemonCommand {
    /// Run the daemon in the foreground
    Run,
    /// Start the daemon in the background (pidfile + logfile in the data dir)
    Start,
    /// Stop a background daemon
    Stop,
    /// Show daemon status (pid + /healthz + /version)
    Status,
    /// Rehearse an upgrade: run this binary against an isolated snapshot of
    /// the data directory (migrations, plugin profile, every manager) without
    /// touching the real state. Prints a JSON report; exit 1 on findings.
    RehearseUpgrade {
        /// Keep the scratch directory even when the rehearsal passes
        #[arg(long)]
        keep: bool,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Print the effective configuration (defaults + file + env)
    Show,
    /// Print the config file path
    Path,
    /// Set one value in config.json (dotted key, JSON or string value)
    Set { key: String, value: String },
    /// Open config.json in $EDITOR
    Edit,
}

type AnyError = Box<dyn std::error::Error + Send + Sync>;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Daemon(DaemonCommand::Run) => daemon_run(),
        Command::Daemon(DaemonCommand::Start) => daemon_start(),
        Command::Daemon(DaemonCommand::Stop) => daemon_stop(),
        Command::Daemon(DaemonCommand::Status) => daemon_status(),
        Command::Daemon(DaemonCommand::RehearseUpgrade { keep }) => daemon_rehearse_upgrade(keep),
        Command::Tui { args } => launch_tui(&args),
        Command::Web { port, dir, no_open } => serve_web(port, dir, no_open),
        Command::Config(ConfigCommand::Show) => config_show(),
        Command::Config(ConfigCommand::Path) => config_path(),
        Command::Config(ConfigCommand::Set { key, value }) => config_set(&key, &value),
        Command::Config(ConfigCommand::Edit) => config_edit(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("kura: {err}");
            ExitCode::FAILURE
        }
    }
}

// ---------------------------------------------------------------------------
// kura daemon …
// ---------------------------------------------------------------------------

fn daemon_run() -> Result<(), AnyError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let config = kura_config::load()?;
        let app = Arc::new(kura_app::App::new(config)?);
        app.serve().await?;
        Ok::<(), AnyError>(())
    })
}

fn daemon_rehearse_upgrade(keep: bool) -> Result<(), AnyError> {
    let config = kura_config::load()?;
    let report = kura_app::rehearse_upgrade(&config, keep)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if report.passed {
        Ok(())
    } else {
        Err(format!("rehearsal failed with {} finding(s)", report.findings.len()).into())
    }
}

fn pid_file(data_dir: &str) -> PathBuf {
    Path::new(data_dir).join("kura.pid")
}

fn running_pid(config: &kura_config::Config) -> Option<i32> {
    let raw = std::fs::read_to_string(pid_file(&config.data_dir)).ok()?;
    let pid: i32 = raw.trim().parse().ok()?;
    // Signal 0 probes liveness without touching the process.
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    alive.then_some(pid)
}

fn daemon_start() -> Result<(), AnyError> {
    let config = kura_config::load()?;
    if let Some(pid) = running_pid(&config) {
        println!(
            "daemon already running (pid {pid}, http://{})",
            config.bind_addr
        );
        return Ok(());
    }
    std::fs::create_dir_all(&config.data_dir)?;
    let log_path = Path::new(&config.data_dir).join("daemon.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let exe = std::env::current_exe()?;
    let child = std::process::Command::new(exe)
        .args(["daemon", "run"])
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()?;
    std::fs::write(pid_file(&config.data_dir), child.id().to_string())?;

    // Wait for readiness (bounded).
    let url = format!("http://{}/healthz", config.bind_addr);
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(200));
        if ureq::get(&url)
            .timeout(Duration::from_secs(1))
            .call()
            .is_ok()
        {
            println!(
                "daemon started (pid {}, http://{}, logs: {})",
                child.id(),
                config.bind_addr,
                log_path.display()
            );
            return Ok(());
        }
    }
    Err(format!(
        "daemon did not become healthy within 10s; check {}",
        log_path.display()
    )
    .into())
}

fn daemon_stop() -> Result<(), AnyError> {
    let config = kura_config::load()?;
    let Some(pid) = running_pid(&config) else {
        println!("daemon is not running");
        let _ = std::fs::remove_file(pid_file(&config.data_dir));
        return Ok(());
    };
    std::process::Command::new("kill")
        .arg(pid.to_string())
        .status()?;
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(200));
        if running_pid(&config).is_none() {
            let _ = std::fs::remove_file(pid_file(&config.data_dir));
            println!("daemon stopped (pid {pid})");
            return Ok(());
        }
    }
    Err(format!("daemon (pid {pid}) did not exit within 10s").into())
}

fn daemon_status() -> Result<(), AnyError> {
    let config = kura_config::load()?;
    let pid = running_pid(&config);
    let health = ureq::get(&format!("http://{}/healthz", config.bind_addr))
        .timeout(Duration::from_secs(2))
        .call()
        .is_ok();
    let version = ureq::get(&format!("http://{}/version", config.bind_addr))
        .timeout(Duration::from_secs(2))
        .call()
        .ok()
        .and_then(|resp| resp.into_json::<serde_json::Value>().ok())
        .and_then(|v| v.get("version").and_then(|s| s.as_str()).map(String::from));
    println!("environment: {:?}", config.environment);
    println!("data dir:    {}", config.data_dir);
    println!("bind addr:   http://{}", config.bind_addr);
    match pid {
        Some(pid) => println!("pid:         {pid} (managed via `kura daemon start`)"),
        None => println!("pid:         - (no pidfile; may be running foreground)"),
    }
    println!("healthy:     {}", if health { "yes" } else { "no" });
    if let Some(version) = version {
        println!("version:     {version}");
    }
    if !health {
        return Err("daemon is not responding".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// kura tui
// ---------------------------------------------------------------------------

fn launch_tui(args: &[String]) -> Result<(), AnyError> {
    // Prefer the canonical sibling binary shipped beside this one.
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("kura-tui")))
        .filter(|path| path.exists());
    let program = sibling.unwrap_or_else(|| PathBuf::from("kura-tui"));
    let status = std::process::Command::new(&program)
        .args(args)
        .status()
        .map_err(|err| {
            format!(
                "launch {}: {err} (is kura-tui installed?)",
                program.display()
            )
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("kura-tui exited with {status}").into())
    }
}

// ---------------------------------------------------------------------------
// kura web
// ---------------------------------------------------------------------------

fn find_web_dist(explicit: Option<PathBuf>) -> Result<PathBuf, AnyError> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = explicit {
        candidates.push(dir);
    }
    if let Ok(env_dir) = std::env::var("KURA_WEB_DIST") {
        candidates.push(PathBuf::from(env_dir));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("web"));
            candidates.push(dir.join("../share/kura/web"));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(Path::new(&home).join(".local/share/kura/web"));
    }
    candidates.push(PathBuf::from("web/dist"));
    for candidate in candidates {
        if candidate.join("index.html").exists() {
            return Ok(candidate);
        }
    }
    Err("web assets not found; pass --dir <path to built web/dist> \
         or set KURA_WEB_DIST (build with `pnpm build:web`)"
        .into())
}

fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn serve_web(port: u16, dir: Option<PathBuf>, no_open: bool) -> Result<(), AnyError> {
    let dist = find_web_dist(dir)?;
    let config = kura_config::load()?;
    let url = format!("http://127.0.0.1:{port}/");
    println!("serving web shell from {} at {url}", dist.display());
    println!(
        "daemon API: http://{} (set inside the shell)",
        config.bind_addr
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let dist = Arc::new(dist);
        let app = axum::Router::new().fallback(axum::routing::get(
            move |request: axum::extract::Request| {
                let dist = dist.clone();
                async move {
                    let rel = request.uri().path().trim_start_matches('/');
                    // SPA: unknown paths fall back to index.html.
                    let mut path = dist.join(rel);
                    if rel.is_empty() || !path.is_file() || rel.contains("..") {
                        path = dist.join("index.html");
                    }
                    match std::fs::read(&path) {
                        Ok(body) => axum::response::Response::builder()
                            .header("content-type", mime_for(&path))
                            .body(axum::body::Body::from(body))
                            .unwrap(),
                        Err(_) => axum::response::Response::builder()
                            .status(404)
                            .body(axum::body::Body::from("not found"))
                            .unwrap(),
                    }
                }
            },
        ));
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
        if !no_open {
            open_browser(&url);
        }
        axum::serve(listener, app).await?;
        Ok::<(), AnyError>(())
    })
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";
    let _ = std::process::Command::new(opener).arg(url).spawn();
}

// ---------------------------------------------------------------------------
// kura config …
// ---------------------------------------------------------------------------

fn config_show() -> Result<(), AnyError> {
    let config = kura_config::load()?;
    println!("{}", serde_json::to_string_pretty(&config)?);
    Ok(())
}

fn effective_config_path() -> Result<PathBuf, AnyError> {
    let config = kura_config::load()?;
    Ok(PathBuf::from(kura_config::config_file_path(
        &config.data_dir,
    )))
}

fn config_path() -> Result<(), AnyError> {
    println!("{}", effective_config_path()?.display());
    Ok(())
}

fn config_set(key: &str, value: &str) -> Result<(), AnyError> {
    let path = effective_config_path()?;
    let mut root: serde_json::Value = match std::fs::read(&path) {
        Ok(raw) => serde_json::from_slice(&raw)
            .map_err(|err| format!("parse {}: {err}", path.display()))?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            serde_json::json!({})
        }
        Err(err) => return Err(err.into()),
    };
    // A JSON literal wins ("true", "123", "{...}"); anything else is a string.
    let parsed: serde_json::Value = serde_json::from_str(value)
        .unwrap_or_else(|_| serde_json::Value::String(value.to_string()));
    let mut cursor = &mut root;
    let segments: Vec<&str> = key.split('.').collect();
    for (i, segment) in segments.iter().enumerate() {
        if i == segments.len() - 1 {
            cursor[segment] = parsed.clone();
        } else {
            if !cursor[segment].is_object() {
                cursor[segment] = serde_json::json!({});
            }
            cursor = &mut cursor[segment];
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&root)?)?;
    std::fs::rename(&tmp, &path)?;
    println!("{} = {} ({})", key, parsed, path.display());
    println!("restart the daemon to apply");
    Ok(())
}

fn config_edit() -> Result<(), AnyError> {
    let path = effective_config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if !path.exists() {
        std::fs::write(&path, b"{}\n")?;
    }
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = std::process::Command::new(&editor).arg(&path).status()?;
    if !status.success() {
        return Err(format!("{editor} exited with {status}").into());
    }
    // Validate the result so a broken edit is caught now, not at boot.
    let raw = std::fs::read(&path)?;
    serde_json::from_slice::<serde_json::Value>(&raw)
        .map_err(|err| format!("config.json is not valid JSON after edit: {err}"))?;
    println!("ok: {}", path.display());
    Ok(())
}
