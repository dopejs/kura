//! Port of daemon/internal/telemetry. See rs/MIGRATION.md for conventions.
//!
//! Minimal structured logger mirroring the Go package: a level-gated text
//! writer (slog TextHandler compatible format) emitting to stdout.

pub mod metrics;

use std::io::Write;
use std::sync::OnceLock;

use chrono::{SecondsFormat, Utc};
use parking_lot::Mutex;

/// Severity gate for emitted records. Ordering matches slog:
/// `Debug < Info < Warn < Error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    /// Parse a config level string. Mirrors Go `New`: unknown or empty
    /// values fall back to `Info`.
    pub fn from_name(name: &str) -> Self {
        match name {
            "debug" => Level::Debug,
            "warn" => Level::Warn,
            "error" => Level::Error,
            _ => Level::Info,
        }
    }

    /// Uppercase label as printed by slog's text handler.
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

/// Level-gated text logger. Equivalent of the Go `telemetry.Logger`
/// wrapping `*slog.Logger` with a stdout `TextHandler`.
pub struct Logger {
    level: Level,
    sink: Mutex<Box<dyn Write + Send>>,
}

impl Logger {
    /// Create a logger writing slog-text records to stdout.
    pub fn new(level: &str) -> Self {
        Self::with_writer(level, std::io::stdout())
    }

    /// Create a logger writing to an arbitrary sink. The Go package
    /// hardcodes `os.Stdout`; this constructor keeps that behavior in
    /// `new` while making the record format and filtering testable.
    pub fn with_writer<W: Write + Send + 'static>(level: &str, writer: W) -> Self {
        Logger {
            level: Level::from_name(level),
            sink: Mutex::new(Box::new(writer)),
        }
    }

    /// Configured minimum level.
    pub fn level(&self) -> Level {
        self.level
    }

    /// Whether a record at `level` would be emitted.
    pub fn enabled(&self, level: Level) -> bool {
        level >= self.level
    }

    /// Emit one record if it passes the level gate. Write errors are
    /// intentionally dropped, matching slog's fire-and-forget semantics.
    pub fn log(&self, level: Level, msg: &str) {
        if !self.enabled(level) {
            return;
        }
        let line = format_record(level, msg);
        let mut sink = self.sink.lock();
        if sink.write_all(line.as_bytes()).is_ok() {
            let _ = sink.flush();
        }
    }

    pub fn debug(&self, msg: &str) {
        self.log(Level::Debug, msg);
    }

    pub fn info(&self, msg: &str) {
        self.log(Level::Info, msg);
    }

    pub fn warn(&self, msg: &str) {
        self.log(Level::Warn, msg);
    }

    pub fn error(&self, msg: &str) {
        self.log(Level::Error, msg);
    }
}

/// Format one record in slog TextHandler layout:
/// `time=<rfc3339-ms> level=<LEVEL> msg=<value>\n`.
fn format_record(level: Level, msg: &str) -> String {
    let ts = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    format!(
        "time={ts} level={} msg={}\n",
        level.as_str(),
        quote_value(msg)
    )
}

/// slog quotes values that are empty or contain spaces, `"`, `=`, or
/// non-printable characters; everything else is emitted bare.
fn quote_value(value: &str) -> String {
    let needs_quote = value.is_empty()
        || value
            .chars()
            .any(|c| c == ' ' || c == '"' || c == '=' || !c.is_ascii_graphic());
    if needs_quote {
        let escaped: String = value
            .chars()
            .flat_map(|c| match c {
                '"' => "\\\"".chars().collect::<Vec<_>>(),
                '\\' => "\\\\".chars().collect::<Vec<_>>(),
                '\n' => "\\n".chars().collect::<Vec<_>>(),
                '\t' => "\\t".chars().collect::<Vec<_>>(),
                c => vec![c],
            })
            .collect();
        format!("\"{escaped}\"")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::Arc;

    /// Shared in-memory sink so tests can inspect emitted records.
    #[derive(Clone, Default)]
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl SharedBuf {
        fn contents(&self) -> String {
            String::from_utf8(self.0.lock().clone()).expect("records are valid UTF-8")
        }
    }

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn logger_at(level: &str) -> (Logger, SharedBuf) {
        let buf = SharedBuf::default();
        (Logger::with_writer(level, buf.clone()), buf)
    }

    #[test]
    fn level_parsing_matches_go_new() {
        assert_eq!(Logger::new("debug").level(), Level::Debug);
        assert_eq!(Logger::new("warn").level(), Level::Warn);
        assert_eq!(Logger::new("error").level(), Level::Error);
        // Go's switch falls through to Info for "info", "", and unknowns.
        assert_eq!(Logger::new("info").level(), Level::Info);
        assert_eq!(Logger::new("").level(), Level::Info);
        assert_eq!(Logger::new("bogus").level(), Level::Info);
        assert_eq!(Logger::new("DEBUG").level(), Level::Info);
    }

    #[test]
    fn records_below_configured_level_are_dropped() {
        let (logger, buf) = logger_at("warn");
        logger.debug("d");
        logger.info("i");
        assert_eq!(buf.contents(), "");

        logger.warn("w");
        logger.error("e");
        let out = buf.contents();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("level=WARN msg=w"));
        assert!(lines[1].contains("level=ERROR msg=e"));
    }

    #[test]
    fn debug_level_emits_everything() {
        let (logger, buf) = logger_at("debug");
        logger.debug("d");
        logger.info("i");
        assert_eq!(buf.contents().lines().count(), 2);
    }

    #[test]
    fn record_format_matches_slog_text_handler() {
        let (logger, buf) = logger_at("info");
        logger.info("hello world");
        let out = buf.contents();
        let line = out.trim_end();
        // time=<rfc3339> level=INFO msg="hello world"
        assert!(line.starts_with("time="), "line: {line}");
        assert!(line.contains("level=INFO"), "line: {line}");
        assert!(line.ends_with("msg=\"hello world\""), "line: {line}");
    }

    #[test]
    fn values_are_quoted_like_slog() {
        assert_eq!(quote_value("plain"), "plain");
        assert_eq!(quote_value(""), "\"\"");
        assert_eq!(quote_value("a b"), "\"a b\"");
        assert_eq!(quote_value("k=v"), "\"k=v\"");
        assert_eq!(quote_value("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(quote_value("a\nb"), "\"a\\nb\"");
    }
}

static GLOBAL_LOGGER: OnceLock<Logger> = OnceLock::new();

/// Installs the process-global logger (Stage 10.3). Later calls are ignored:
/// the first installer (the daemon's serve path) wins, so a test cannot
/// redirect production output.
pub fn install_global_logger(logger: Logger) {
    let _ = GLOBAL_LOGGER.set(logger);
}

/// The process-global logger; an info-level stdout logger until installed.
pub fn global_logger() -> &'static Logger {
    GLOBAL_LOGGER.get_or_init(|| Logger::new("info"))
}

/// One structured access-log record (Stage 10.3): slog text layout with the
/// request id, tenant, route template, status and latency, so a request can
/// be correlated across the daemon's log and the `x-request-id` a client
/// saw. `tenant` is empty for single-user and unauthenticated requests.
pub fn access_log(
    request_id: &str,
    tenant: &str,
    method: &str,
    route: &str,
    status: u16,
    duration_ms: u128,
) {
    let logger = global_logger();
    if !logger.enabled(Level::Info) {
        return;
    }
    logger.info(&format!(
        "http.request request_id={} tenant={} method={} route={} status={} duration_ms={}",
        quote_value(request_id),
        quote_value(tenant),
        method,
        quote_value(route),
        status,
        duration_ms
    ));
}
