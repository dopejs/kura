mod client;
mod commands;
mod persist;
mod render;

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::Parser;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    crossterm::{
        event::{self, Event, KeyCode, KeyModifiers},
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    },
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use tokio::sync::mpsc;

use client::{ChatQueryInput, ChatQueryResponse, Client};

/// Kura full-screen terminal client.
#[derive(Parser, Debug)]
#[command(name = "kura-tui", version)]
struct Cli {
    /// Daemon base URL.
    #[arg(
        long,
        env = "KURA_DAEMON_URL",
        default_value = "http://127.0.0.1:19192"
    )]
    daemon_url: String,
    /// Access token for daemon auth.
    #[arg(long, env = "KURA_ACCESS_TOKEN")]
    token: Option<String>,
    /// Optional provider override.
    #[arg(long)]
    provider: Option<String>,
    /// Optional model override.
    #[arg(long)]
    model: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Role {
    User,
    Assistant,
    System,
    Error,
    Event,
}

struct Message {
    role: Role,
    content: String,
    done: bool,
}

struct Picker {
    title: String,
    items: Vec<(String, String)>,
    index: usize,
}

struct App {
    daemon_url: String,
    provider: Option<String>,
    model: Option<String>,
    thread_id: Option<String>,
    input: String,
    messages: Vec<Message>,
    history: Vec<String>,
    history_index: i32,
    scroll_offset: usize,
    busy: bool,
    picker: Option<Picker>,
    pending_editor: bool,
    event_stop: Option<Arc<AtomicBool>>,
}

enum StreamMsg {
    Delta(String),
    Done(Result<ChatQueryResponse, String>),
    Event(String, serde_json::Value),
    Command(commands::CommandResult),
}

impl App {
    fn push(&mut self, role: Role, content: String, done: bool) {
        self.messages.push(Message {
            role,
            content,
            done,
        });
        self.scroll_offset = 0;
    }

    fn status(&self) -> String {
        format!(
            "{}  provider={} model={} thread={}",
            self.daemon_url,
            self.provider.as_deref().unwrap_or("default"),
            self.model.as_deref().unwrap_or("default"),
            self.thread_id.as_deref().unwrap_or("new"),
        )
    }
}

fn draw(frame: &mut ratatui::Frame, app: &App) {
    if let Some(picker) = &app.picker {
        let mut lines: Vec<Line<'static>> = vec![Line::from(Span::styled(
            picker.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ))];
        for (i, (_, label)) in picker.items.iter().enumerate() {
            let style = if i == picker.index {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            let prefix = if i == picker.index { "\u{276f} " } else { "  " };
            lines.push(Line::from(Span::styled(format!("{prefix}{label}"), style)));
        }
        lines.push(Line::from(Span::styled(
            "Enter select \u{b7} Esc cancel",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(Paragraph::new(lines), frame.area());
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new(app.status()).style(Style::default().fg(Color::DarkGray)),
        chunks[0],
    );

    let visible_count = chunks[1].height.saturating_sub(1) as usize;
    let total = app.messages.len();
    let start = if app.scroll_offset >= total {
        0
    } else {
        total - app.scroll_offset - visible_count.min(total - app.scroll_offset)
    };
    let visible = &app.messages[start..];

    let mut body: Vec<Line<'static>> = Vec::new();
    for m in visible {
        match m.role {
            Role::User => {
                let mut spans = vec![Span::styled(
                    "> ",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )];
                spans.push(Span::styled(
                    m.content.clone(),
                    Style::default().fg(Color::Green),
                ));
                body.push(Line::from(spans));
            }
            Role::Assistant => body.extend(render::markdown_lines(&m.content)),
            Role::Error => {
                body.push(Line::from(Span::styled(
                    format!("[error] {}", m.content),
                    Style::default().fg(Color::Red),
                )));
            }
            Role::Event => {
                body.push(Line::from(Span::styled(
                    m.content.clone(),
                    Style::default().fg(Color::Yellow),
                )));
            }
            Role::System => {
                body.push(Line::from(Span::styled(
                    m.content.clone(),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
    }
    if app.busy {
        body.push(Line::from(Span::styled(
            "\u{258c}",
            Style::default().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(Paragraph::new(body), chunks[1]);

    let prompt = format!("\u{276f} {}", app.input);
    frame.render_widget(
        Paragraph::new(prompt).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Blue)),
        ),
        chunks[2],
    );
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let client = Arc::new(Client::new(cli.daemon_url.clone(), cli.token.clone()));
    let mut app = App {
        daemon_url: cli.daemon_url,
        provider: cli.provider,
        model: cli.model,
        thread_id: None,
        input: String::new(),
        messages: vec![Message {
            role: Role::System,
            content: "Kura (type /help)".to_string(),
            done: true,
        }],
        history: Vec::new(),
        history_index: -1,
        scroll_offset: 0,
        busy: false,
        picker: None,
        pending_editor: false,
        event_stop: None,
    };

    // restore last session
    if let Some(state) = persist::load() {
        app.messages = state
            .messages
            .into_iter()
            .map(|m| Message {
                role: role_from_str(&m.role),
                content: m.content,
                done: true,
            })
            .collect();
        app.provider = state.provider;
        app.model = state.model;
        app.thread_id = state.thread_id;
        if app.messages.is_empty() {
            app.push(Role::System, "Kura (type /help)".to_string(), true);
        }
    }

    // crossterm events -> channel (blocking read on a worker thread)
    let (event_tx, mut event_rx) = mpsc::channel::<Event>(32);
    tokio::task::spawn_blocking(move || {
        while let Ok(ev) = event::read() {
            if event_tx.blocking_send(ev).is_err() {
                break;
            }
        }
    });

    // streaming deltas -> channel
    let (stream_tx, mut stream_rx) = mpsc::channel::<StreamMsg>(64);

    loop {
        if app.pending_editor {
            app.pending_editor = false;
            let mut input = app.input.clone();
            match open_external_editor(&mut terminal, &mut input) {
                Ok(()) => app.input = input,
                Err(e) => app.push(Role::Error, format!("editor error: {e}"), true),
            }
        }
        terminal.draw(|f| draw(f, &app))?;

        tokio::select! {
            Some(ev) = event_rx.recv() => {
                let quit = handle_key(&mut app, ev, &client, &stream_tx);
                if quit { break; }
            }
            Some(msg) = stream_rx.recv() => {
                match msg {
                    StreamMsg::Delta(reply) => {
                        if let Some(last) = app.messages.last_mut() {
                            last.content = reply;
                        }
                    }
                    StreamMsg::Done(result) => {
                        app.busy = false;
                        if let Some(last) = app.messages.last_mut() {
                            last.done = true;
                        }
                        match result {
                            Ok(resp) => {
                                if resp.thread_id.is_some() {
                                    app.thread_id = resp.thread_id;
                                }
                                if let Some(last) = app.messages.last_mut() {
                                    if last.content.is_empty() { last.content = resp.reply; }
                                }
                            }
                            Err(e) => {
                                app.push(Role::Error, e, true);
                            }
                        }
                    }
                    StreamMsg::Event(name, value) => {
                        app.push(Role::Event, format_event(&name, &value), true);
                    }
                    StreamMsg::Command(result) => {
                        match result {
                            commands::CommandResult::Push(role, content) => app.push(role, content, true),
                            commands::CommandResult::SetModel(m) => app.model = m,
                            commands::CommandResult::SetProvider(p) => app.provider = p,
                            commands::CommandResult::SetThread(t) => app.thread_id = t,
                            commands::CommandResult::OpenThreadPicker { title, items } => {
                                app.picker = Some(Picker { title, items, index: 0 });
                            }
                            commands::CommandResult::ToggleEvents => toggle_events(&mut app, &client, &stream_tx),
                            commands::CommandResult::Quit => break,
                        }
                    }
                }
            }
        }
    }

    let _ = persist::save(&persist::PersistedState {
        messages: app
            .messages
            .iter()
            .map(|m| persist::PersistedMessage {
                role: role_to_str(m.role).to_string(),
                content: m.content.clone(),
            })
            .collect(),
        provider: app.provider.clone(),
        model: app.model.clone(),
        thread_id: app.thread_id.clone(),
    });

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

fn handle_key(
    app: &mut App,
    ev: Event,
    client: &Arc<Client>,
    stream_tx: &mpsc::Sender<StreamMsg>,
) -> bool {
    let Event::Key(key) = ev else { return false };
    if app.picker.is_some() {
        match key.code {
            KeyCode::Esc => {
                app.picker = None;
            }
            KeyCode::Up => {
                if let Some(p) = &mut app.picker {
                    p.index = p.index.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if let Some(p) = &mut app.picker {
                    p.index = (p.index + 1).min(p.items.len().saturating_sub(1));
                }
            }
            KeyCode::Enter => {
                if let Some(p) = app.picker.take() {
                    if let Some((value, _)) = p.items.get(p.index) {
                        app.thread_id = Some(value.clone());
                        app.push(Role::System, format!("thread = {value}"), true);
                    }
                }
            }
            _ => {}
        }
        return false;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return true;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('l') {
        app.messages.clear();
        app.scroll_offset = 0;
        return false;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('x') {
        app.pending_editor = true;
        return false;
    }
    match key.code {
        KeyCode::Esc => {
            if app.busy {
                app.busy = false;
            } else {
                return true;
            }
        }
        KeyCode::Enter => {
            if app.input.ends_with('\\') {
                app.input.pop();
                app.input.push('\n');
            } else {
                submit(app, client, stream_tx);
            }
        }
        KeyCode::Backspace => {
            app.input.pop();
        }
        KeyCode::Up => {
            if app.history.is_empty() {
                return false;
            }
            app.history_index = if app.history_index < 0 {
                app.history.len() as i32 - 1
            } else {
                (app.history_index - 1).max(0)
            };
            app.input = app.history[app.history_index as usize].clone();
        }
        KeyCode::Down => {
            if app.history_index < 0 {
                return false;
            }
            app.history_index += 1;
            if app.history_index as usize >= app.history.len() {
                app.history_index = -1;
                app.input.clear();
            } else {
                app.input = app.history[app.history_index as usize].clone();
            }
        }
        KeyCode::PageUp => {
            app.scroll_offset = app.scroll_offset.saturating_add(10);
        }
        KeyCode::PageDown => {
            app.scroll_offset = app.scroll_offset.saturating_sub(10);
        }
        KeyCode::Tab => {
            if app.input.starts_with('/') {
                const CMDS: &[&str] = &[
                    "/help",
                    "/exit",
                    "/quit",
                    "/model",
                    "/provider",
                    "/thread",
                    "/threads",
                    "/reset",
                    "/workspaces",
                    "/bindings",
                    "/profiles",
                    "/connectors",
                    "/tenants",
                    "/me",
                    "/config",
                    "/events",
                ];
                let matches: Vec<&str> = CMDS
                    .iter()
                    .copied()
                    .filter(|c| c.starts_with(app.input.as_str()))
                    .collect();
                match matches.len() {
                    1 => app.input = format!("{} ", matches[0]),
                    n if n > 1 => {
                        let common = common_prefix(&matches);
                        if common.len() > app.input.len() {
                            app.input = common;
                        } else {
                            app.push(
                                Role::System,
                                format!("commands: {}", matches.join("  ")),
                                true,
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
        KeyCode::Char(c) => app.input.push(c),
        _ => {}
    }
    false
}

fn submit(app: &mut App, client: &Arc<Client>, stream_tx: &mpsc::Sender<StreamMsg>) {
    let text = app.input.trim().to_string();
    if text.is_empty() || app.busy {
        return;
    }
    app.input.clear();
    app.history.push(text.clone());
    app.history_index = -1;

    if text.starts_with('/') {
        let (cmd, args) = text
            .split_once(' ')
            .map(|(c, a)| (c.to_string(), a.trim().to_string()))
            .unwrap_or((text.clone(), String::new()));
        let client = client.clone();
        let tx = stream_tx.clone();
        tokio::spawn(async move {
            let result = commands::run_command(&cmd, &args, &client).await;
            let _ = tx.send(StreamMsg::Command(result)).await;
        });
        return;
    }

    app.push(Role::User, text.clone(), true);
    app.push(Role::Assistant, String::new(), false);
    app.busy = true;

    let input = ChatQueryInput {
        query: text,
        provider: app.provider.clone(),
        model: app.model.clone(),
        thread_id: app.thread_id.clone(),
    };
    let client = client.clone();
    let tx = stream_tx.clone();
    tokio::spawn(async move {
        let result = client
            // A dropped frame is superseded by the next: each carries the
            // round's whole text so far.
            .stream_chat(&input, |reply| {
                let _ = tx.try_send(StreamMsg::Delta(reply));
            })
            .await;
        let _ = tx.send(StreamMsg::Done(result)).await;
    });
}

fn role_to_str(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
        Role::Error => "error",
        Role::Event => "event",
    }
}

fn role_from_str(s: &str) -> Role {
    match s {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        "error" => Role::Error,
        "event" => Role::Event,
        _ => Role::System,
    }
}

fn common_prefix(items: &[&str]) -> String {
    let mut prefix = items[0].to_string();
    for item in items.iter().skip(1) {
        while !item.starts_with(&prefix) {
            prefix.pop();
        }
        if prefix.is_empty() {
            break;
        }
    }
    prefix
}

fn toggle_events(app: &mut App, client: &Arc<Client>, stream_tx: &mpsc::Sender<StreamMsg>) {
    if let Some(stop) = app.event_stop.take() {
        stop.store(false, Ordering::SeqCst);
        app.push(Role::System, "events stream stopped".to_string(), true);
        return;
    }
    let running = Arc::new(AtomicBool::new(true));
    app.event_stop = Some(running.clone());
    let client = client.clone();
    let tx = stream_tx.clone();
    tokio::spawn(async move {
        let result = client
            .stream_events(|name, value| {
                let _ = tx.try_send(StreamMsg::Event(name, value));
                running.load(Ordering::SeqCst)
            })
            .await;
        if let Err(e) = result {
            let _ = tx
                .send(StreamMsg::Command(commands::CommandResult::Push(
                    Role::Error,
                    format!("[events] {e}"),
                )))
                .await;
        }
    });
    app.push(
        Role::System,
        "events stream started (/events to stop)".to_string(),
        true,
    );
}

fn format_event(name: &str, value: &serde_json::Value) -> String {
    let category = value
        .get("category")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| name.split('.').next().unwrap_or("event"));
    let mut parts: Vec<String> = Vec::new();
    if let Some(payload) = value.get("payload").and_then(|p| p.as_object()) {
        for key in ["toolName", "title", "entrypoint", "kind"] {
            if let Some(v) = payload.get(key).and_then(|v| v.as_str()) {
                if !v.is_empty() {
                    parts.push(v.to_string());
                    break;
                }
            }
        }
        if let Some(status) = payload.get("status").and_then(|v| v.as_str()) {
            parts.push(status.to_string());
        }
    }
    if parts.is_empty() {
        format!("[{category}] {name}")
    } else {
        format!("[{category}] {name}  {}", parts.join("  "))
    }
}

fn open_external_editor(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    input: &mut String,
) -> io::Result<()> {
    let path = std::env::temp_dir().join("kura-tui-input.txt");
    std::fs::write(&path, input.as_bytes())?;
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let _ = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{} \"{}\"", editor, path.display()))
        .status();
    if let Ok(content) = std::fs::read_to_string(&path) {
        *input = content;
    }
    enable_raw_mode()?;
    execute!(terminal.backend_mut(), EnterAlternateScreen)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_event_step_created() {
        let value = serde_json::json!({
            "category": "step",
            "name": "step.created",
            "payload": { "title": "reply to connector message", "kind": "chat_query", "status": "queued" }
        });
        let out = format_event("step.created", &value);
        assert!(out.starts_with("[step] step.created"), "got {out:?}");
        assert!(out.contains("reply to connector message"), "got {out:?}");
        assert!(out.contains("queued"), "got {out:?}");
    }

    #[test]
    fn format_event_tool_call_requested() {
        let value = serde_json::json!({
            "category": "tool_call",
            "name": "tool_call.requested",
            "payload": { "toolName": "exec-skill", "status": "requested" }
        });
        let out = format_event("tool_call.requested", &value);
        assert!(
            out.starts_with("[tool_call] tool_call.requested"),
            "got {out:?}"
        );
        assert!(out.contains("exec-skill"), "got {out:?}");
    }

    #[test]
    fn role_event_roundtrip() {
        assert_eq!(role_from_str(role_to_str(Role::Event)), Role::Event);
    }
}
