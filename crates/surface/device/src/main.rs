use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// Lets a Kura agent use this computer, only while you allow it.
#[derive(Parser)]
#[command(name = "kura-device", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Pair with a Kura server using a code from `POST /v1/devices/pairings`.
    Pair {
        /// The pairing code, `kpair.…`; valid for 10 minutes, once.
        code: String,
        /// The Kura server, e.g. https://kura.example.com
        #[arg(long)]
        server: String,
        /// How this computer is listed; defaults to its host name.
        #[arg(long)]
        name: Option<String>,
    },
    /// Connect and serve the agent until stopped (Ctrl-C).
    Run {
        /// Grant sessions without asking for this long, e.g. `30m`.
        /// Default: ask every time.
        #[arg(long)]
        trust_for: Option<String>,
        /// Another app (bundle id) to never click, type into, or show.
        #[arg(long = "deny-app")]
        deny_apps: Vec<String>,
    },
    /// Check the permissions the device needs and where to grant them.
    Doctor {
        /// Also open the System Settings panes that are missing a grant.
        #[arg(long)]
        open: bool,
    },
    /// Show the pairing, the permissions, and whether `run` is running.
    Status,
    /// The kill switch: end the agent's session now and refuse new ones for
    /// 10 minutes (same as Control-Option-Command-Period).
    Stop,
    /// Forget the pairing on this computer. Revoke the device in Kura too.
    Unpair,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("kura-device: {err}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(app::dispatch(cli.command)) {
        Ok(()) => ExitCode::SUCCESS,
        Err((code, message)) => {
            eprintln!("kura-device: {message}");
            ExitCode::from(code)
        }
    }
}

#[cfg(target_os = "macos")]
mod app {
    use std::sync::Arc;
    use std::time::Duration;

    use kura_device::agent::{self, Activity, Agent, Policy};
    use kura_device::config::{self, Config, Keychain, TokenStore as _};
    use kura_device::link::{self, Ended, LinkConfig};
    use kura_device::platform::macos;
    use kura_device::{log, pair};
    use tokio::sync::mpsc;

    use super::Command;

    type Failure = (u8, String);

    fn fail(message: impl Into<String>) -> Failure {
        (1, message.into())
    }

    pub async fn dispatch(command: Command) -> Result<(), Failure> {
        let home = config::home();
        match command {
            Command::Pair { code, server, name } => {
                let server = server.trim_end_matches('/').to_owned();
                link::connect_url(&server).map_err(fail)?;
                let name = name.unwrap_or_else(host_name);
                let name: String = name.chars().take(64).collect();
                let paired = pair::pair(&server, &code, &name).await.map_err(fail)?;
                Keychain.set(&server, &paired.token).map_err(fail)?;
                config::save(
                    &home,
                    &Config {
                        server: server.clone(),
                        device_id: paired.device_id.clone(),
                        name,
                    },
                )
                .map_err(fail)?;
                println!("Paired as {} with {server}.", paired.device_id);
                println!("Next: `kura-device doctor`, then `kura-device run`.");
                Ok(())
            }
            Command::Run {
                trust_for,
                deny_apps,
            } => run(&home, trust_for, deny_apps).await,
            Command::Doctor { open } => {
                let ready = doctor(open);
                if ready {
                    Ok(())
                } else {
                    Err(fail("permissions missing"))
                }
            }
            Command::Status => {
                match config::load(&home).map_err(fail)? {
                    Some(config) => {
                        let token = Keychain.get(&config.server).map_err(fail)?.is_some();
                        println!(
                            "paired:   {} as {} ({})",
                            config.server, config.device_id, config.name
                        );
                        println!(
                            "token:    {}",
                            if token {
                                "in Keychain"
                            } else {
                                "MISSING, pair again"
                            }
                        );
                    }
                    None => println!("paired:   no (kura-device pair <code> --server <url>)"),
                }
                match config::running_pid(&home) {
                    Some(pid) => println!("running:  yes, pid {pid}"),
                    None => println!("running:  no"),
                }
                doctor(false);
                Ok(())
            }
            Command::Stop => {
                let pid = config::running_pid(&home).ok_or_else(|| fail("not running"))?;
                // SAFETY: plain signal to a pid we read from our own pid file.
                if unsafe { libc::kill(pid, libc::SIGUSR1) } != 0 {
                    return Err(fail(format!("could not signal pid {pid}")));
                }
                println!("Stopped: any session ended; new ones refused for 10 minutes.");
                Ok(())
            }
            Command::Unpair => {
                if let Some(config) = config::load(&home).map_err(fail)? {
                    Keychain.delete(&config.server).map_err(fail)?;
                    config::remove(&home).map_err(fail)?;
                    println!(
                        "Forgot {} on this computer. Revoke it in Kura as well \
                         (POST /v1/devices/{}/revoke) so the token is dead everywhere.",
                        config.device_id, config.device_id
                    );
                } else {
                    println!("Not paired.");
                }
                Ok(())
            }
        }
    }

    async fn run(
        home: &std::path::Path,
        trust_for: Option<String>,
        deny_apps: Vec<String>,
    ) -> Result<(), Failure> {
        let config = config::load(home)
            .map_err(fail)?
            .ok_or_else(|| fail("not paired: kura-device pair <code> --server <url>"))?;
        let token = Keychain
            .get(&config.server)
            .map_err(fail)?
            .ok_or_else(|| fail("no token in the Keychain: pair again"))?;
        if let Some(pid) = config::running_pid(home) {
            return Err(fail(format!("already running as pid {pid}")));
        }
        if !doctor(false) {
            return Err(fail("permissions missing; see above"));
        }
        let _ = log::to_file(&config::log_file());

        let mut policy = Policy::default();
        policy.deny_apps.extend(deny_apps);
        if let Some(text) = trust_for {
            let window = parse_duration(&text).map_err(fail)?;
            policy.trust_until_ms = now_ms() + window.as_millis() as u64;
            log::warn(&format!(
                "sessions are granted without asking for {}s",
                window.as_secs()
            ));
        }

        let activity = Arc::new(Activity::default());
        let (kill_tx, mut kills) = mpsc::channel(4);
        // Fail closed: no watcher means no kill switch and no take-over.
        macos::watch(activity.clone(), kill_tx.clone()).map_err(|err| fail(err.to_string()))?;
        let platform = macos::Mac::new(home.join("tmp")).map_err(|err| fail(err.to_string()))?;
        let agent = Arc::new(Agent::new(
            Arc::new(platform),
            Arc::new(macos::Alert),
            activity,
            policy,
            agent::system_clock(),
        ));

        let pid_file = config::pid_file(home);
        std::fs::write(&pid_file, std::process::id().to_string())
            .map_err(|err| fail(format!("{}: {err}", pid_file.display())))?;

        let (shutdown_tx, mut shutdown) = mpsc::channel(1);
        tokio::spawn(signals(kill_tx, shutdown_tx));
        let url = link::connect_url(&config.server).map_err(fail)?;
        log::info(&format!(
            "serving {} as {}",
            config.server, config.device_id
        ));
        let ended = link::run(
            agent,
            url,
            token,
            LinkConfig::default(),
            &mut kills,
            &mut shutdown,
        )
        .await;
        let _ = std::fs::remove_file(&pid_file);
        match ended {
            Ended::Shutdown => {
                log::info("stopped");
                Ok(())
            }
            Ended::Unauthorized => Err((
                2,
                "the server refused this device's token (revoked or unknown); pair again".into(),
            )),
            Ended::Lost(why) => Err(fail(why)),
        }
    }

    async fn signals(kills: mpsc::Sender<()>, shutdown: mpsc::Sender<()>) {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut usr1), Ok(mut term), Ok(mut int)) = (
            signal(SignalKind::user_defined1()),
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
        ) else {
            log::warn("signal handlers unavailable; `kura-device stop` will not work");
            return;
        };
        loop {
            tokio::select! {
                _ = usr1.recv() => { let _ = kills.try_send(()); }
                _ = term.recv() => { let _ = shutdown.try_send(()); }
                _ = int.recv() => { let _ = shutdown.try_send(()); }
            }
        }
    }

    /// Prints each permission; returns whether all are granted.
    fn doctor(open: bool) -> bool {
        let checks = [
            (
                "Screen Recording",
                macos::capture_allowed(),
                macos::SCREEN_RECORDING_PANE,
            ),
            (
                "Accessibility",
                macos::input_allowed(),
                macos::ACCESSIBILITY_PANE,
            ),
        ];
        let mut ready = true;
        for (name, granted, pane) in checks {
            if granted {
                println!("{name:<17} granted");
                continue;
            }
            ready = false;
            println!("{name:<17} MISSING: System Settings › Privacy & Security › {name}");
            println!(
                "{:<17} grant it to the app that runs kura-device (your terminal, or kura-device itself)",
                ""
            );
            if open {
                if name == "Screen Recording" {
                    macos::request_capture();
                }
                let _ = std::process::Command::new("/usr/bin/open")
                    .arg(pane)
                    .status();
            }
        }
        ready
    }

    fn host_name() -> String {
        let mut buf = [0u8; 256];
        // SAFETY: the buffer and its length are valid.
        let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
        let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
        let name = String::from_utf8_lossy(&buf[..end]);
        let name = name.trim_end_matches(".local");
        if ok && !name.is_empty() {
            name.to_owned()
        } else {
            "mac".into()
        }
    }

    fn now_ms() -> u64 {
        (agent::system_clock())()
    }

    /// `90s`, `30m`, `2h`; at most 12 hours.
    fn parse_duration(text: &str) -> Result<Duration, String> {
        let text = text.trim();
        let (number, unit) = text.split_at(text.len().saturating_sub(1));
        let n: u64 = number
            .parse()
            .map_err(|_| format!("`{text}`: use e.g. 90s, 30m or 2h"))?;
        let secs = match unit {
            "s" => n,
            "m" => n * 60,
            "h" => n * 3600,
            _ => return Err(format!("`{text}`: use e.g. 90s, 30m or 2h")),
        };
        if secs == 0 || secs > 12 * 3600 {
            return Err("trust window must be between 1s and 12h".into());
        }
        Ok(Duration::from_secs(secs))
    }
}

#[cfg(not(target_os = "macos"))]
mod app {
    use super::Command;

    pub async fn dispatch(_command: Command) -> Result<(), (u8, String)> {
        Err((1, "kura-device supports macOS only for now".into()))
    }
}
