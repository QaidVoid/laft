//! Human and JSON output: phase lines, symbols, colors, tables, numbers,
//! times, confirmation and the JSON documents and events.

use std::fmt::Write as _;
use std::io::{self, BufRead, IsTerminal, Write};
use std::time::Duration;

use laft_core::Timestamp;
use serde_json::{Value, json};

use crate::cli::{ColorChoice, Global, MessageFormat};
use crate::failure::{Failure, exit};

/// The version of the JSON output.
pub(crate) const LAFT_JSON: u32 = 1;

/// What a symbol of human output means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sym {
    /// Something was added.
    Added,
    /// Something was removed.
    Removed,
    /// A package moved to a newer version.
    Upgraded,
    /// A package moved to an older version.
    Downgraded,
    /// Something changed.
    Changed,
    /// A service was restarted.
    Restarted,
    /// Nothing changed.
    Unchanged,
    /// A check passed.
    Pass,
    /// A check failed.
    Fail,
}

/// A color of human output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Paint {
    Green,
    Red,
    Yellow,
    Magenta,
    Blue,
    Dim,
    BoldRed,
    BoldYellow,
}

impl Paint {
    fn ansi(self) -> &'static str {
        match self {
            Self::Green => "32",
            Self::Red => "31",
            Self::Yellow => "33",
            Self::Magenta => "35",
            Self::Blue => "34",
            Self::Dim => "2",
            Self::BoldRed => "1;31",
            Self::BoldYellow => "1;33",
        }
    }
}

impl Sym {
    fn glyph(self, ascii: bool) -> &'static str {
        match (self, ascii) {
            (Self::Added, _) => "+",
            (Self::Removed, _) => "-",
            (Self::Upgraded, false) => "\u{2191}",
            (Self::Upgraded, true) => "^",
            (Self::Downgraded, false) => "\u{2193}",
            (Self::Downgraded, true) => "v",
            (Self::Changed, _) => "~",
            (Self::Restarted, false) => "\u{21bb}",
            (Self::Restarted, true) => "r",
            (Self::Unchanged, _) => "=",
            (Self::Pass, false) => "\u{2713}",
            (Self::Pass, true) => "ok",
            (Self::Fail, false) => "\u{2717}",
            (Self::Fail, true) => "FAIL",
        }
    }

    fn paint(self) -> Paint {
        match self {
            Self::Added | Self::Pass => Paint::Green,
            Self::Removed | Self::Fail => Paint::Red,
            Self::Upgraded | Self::Changed => Paint::Yellow,
            Self::Downgraded => Paint::Magenta,
            Self::Restarted => Paint::Blue,
            Self::Unchanged => Paint::Dim,
        }
    }
}

/// How the command talks to the user.
#[derive(Debug, Clone)]
pub(crate) struct Ui {
    /// Whether output is JSON.
    pub(crate) json: bool,
    /// Whether human output uses ASCII symbols only.
    pub(crate) ascii: bool,
    /// Whether only errors and the final result are printed.
    pub(crate) quiet: bool,
    /// How much more detail to print.
    pub(crate) verbose: u8,
    /// Whether confirmations are assumed.
    pub(crate) yes: bool,
    /// How configuration diagnostics are written.
    pub(crate) message_format: MessageFormat,
    color_out: bool,
    color_err: bool,
    interactive: bool,
    stderr_tty: bool,
    pager: bool,
    events: std::cell::Cell<bool>,
}

impl Ui {
    /// The output settings of `global`, the environment and the terminals.
    pub(crate) fn new(global: &Global) -> Self {
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        let choice = global.color.or_else(|| match env("LAFT_COLOR").as_deref() {
            Some("always") => Some(ColorChoice::Always),
            Some("never") => Some(ColorChoice::Never),
            Some("auto") => Some(ColorChoice::Auto),
            _ => None,
        });
        let no_color = env("NO_COLOR").is_some();
        let colored = |tty: bool| match choice {
            Some(ColorChoice::Always) => true,
            Some(ColorChoice::Never) => false,
            _ => tty && !no_color,
        };
        let stdout_tty = io::stdout().is_terminal();
        let stderr_tty = io::stderr().is_terminal();
        let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
            .iter()
            .find_map(|name| env(name))
            .unwrap_or_default()
            .to_ascii_lowercase();
        let utf8 = locale.contains("utf-8") || locale.contains("utf8");
        Self {
            json: global.json,
            ascii: global.ascii || !utf8,
            quiet: global.quiet,
            verbose: global.verbose,
            yes: global.yes,
            message_format: global.message_format,
            color_out: colored(stdout_tty) && !global.json,
            color_err: colored(stderr_tty),
            // The question goes to standard error and the answer comes
            // from standard input, so standard output may be redirected.
            interactive: stderr_tty && io::stdin().is_terminal(),
            stderr_tty,
            pager: stdout_tty && !global.no_pager && !global.json,
            events: std::cell::Cell::new(false),
        }
    }

    /// `text` in `paint`, for standard output.
    pub(crate) fn paint(&self, paint: Paint, text: &str) -> String {
        paint_if(self.color_out, paint, text)
    }

    /// `text` in `paint`, for standard error.
    pub(crate) fn paint_err(&self, paint: Paint, text: &str) -> String {
        paint_if(self.color_err, paint, text)
    }

    /// The symbol for `sym`, colored for standard output.
    pub(crate) fn sym(&self, sym: Sym) -> String {
        self.paint(sym.paint(), sym.glyph(self.ascii))
    }

    /// The symbol for `sym`, colored for standard error.
    pub(crate) fn sym_err(&self, sym: Sym) -> String {
        self.paint_err(sym.paint(), sym.glyph(self.ascii))
    }

    /// The arrow between an old and a new value.
    pub(crate) fn arrow(&self) -> &'static str {
        if self.ascii { "->" } else { "\u{2192}" }
    }

    /// The separator between facts on one line.
    pub(crate) fn dot(&self) -> &'static str {
        if self.ascii { "-" } else { "\u{b7}" }
    }

    /// Writes a result line to standard output.
    pub(crate) fn out(&self, text: impl AsRef<str>) {
        let _ = writeln!(io::stdout().lock(), "{}", text.as_ref());
    }

    /// Writes a block of result lines to standard output, through the pager
    /// when it is taller than the terminal.
    pub(crate) fn page(&self, text: &str) {
        if self.pager && taller_than_screen(text) && page(text) {
            return;
        }
        let _ = io::stdout().lock().write_all(text.as_bytes());
    }

    /// Writes a phase line to standard error, unless output is JSON or
    /// quiet.
    pub(crate) fn phase(&self, label: &str, detail: impl AsRef<str>) {
        if !self.json && !self.quiet {
            eprintln!("{label:<11} {}", detail.as_ref());
        }
    }

    /// Writes a line to standard error, unless output is JSON or quiet.
    pub(crate) fn note(&self, text: impl AsRef<str>) {
        if !self.json && !self.quiet {
            eprintln!("{}", text.as_ref());
        }
    }

    /// Writes a warning to standard error, unless output is JSON.
    pub(crate) fn warn(&self, text: impl AsRef<str>) {
        if !self.json {
            let mut lines = text.as_ref().lines();
            let first = lines.next().unwrap_or_default();
            eprintln!("{}: {first}", self.paint_err(Paint::BoldYellow, "warning"));
            for line in lines {
                eprintln!("  {line}");
            }
        }
    }

    /// Writes a warning with a diagnostic code to standard error, unless
    /// output is JSON.
    pub(crate) fn warn_code(&self, code: &str, text: impl AsRef<str>) {
        if !self.json {
            let head = self.paint_err(Paint::BoldYellow, &format!("warning[{code}]"));
            eprintln!("{head}: {}", text.as_ref());
        }
    }

    /// Writes progress that is rewritten in place on a terminal.
    pub(crate) fn progress(&self, label: &str, done: u64, total: u64) {
        if self.json || self.quiet || !self.stderr_tty {
            return;
        }
        let width = 30;
        let filled = (done.min(total) * width as u64)
            .checked_div(total)
            .map_or(width, |f| f as usize);
        let (full, empty) = if self.ascii {
            ("#", "-")
        } else {
            ("\u{2501}", "\u{2500}")
        };
        eprint!(
            "\r{label:<11} {}{}  {}/{}\x1b[K",
            full.repeat(filled),
            empty.repeat(width - filled),
            size(done),
            size(total)
        );
        if done >= total {
            eprint!("\r\x1b[K");
        }
    }

    /// Prints a query document with `"laft_json": 1` added.
    pub(crate) fn doc(&self, mut value: Value) {
        if let Value::Object(map) = &mut value {
            map.insert("laft_json".into(), json!(LAFT_JSON));
        }
        self.out(value.to_string());
    }

    /// Prints one event of a long-running command, in JSON mode only and
    /// after [`Self::start`].
    pub(crate) fn event(&self, value: Value) {
        if self.json && self.events.get() {
            self.out(value.to_string());
        }
    }

    /// Prints the `start` event of a long-running command, which turns on
    /// the events that follow.
    pub(crate) fn start(&self, command: &str) {
        self.events.set(true);
        self.event(json!({
            "event": "start",
            "laft_json": LAFT_JSON,
            "command": command,
            "pid": std::process::id(),
        }));
    }

    /// Asks `question` and returns when the answer is yes.
    ///
    /// # Errors
    ///
    /// Fails with status 2 when nobody can be asked and `-y` was not given,
    /// and with status 10 when the answer is no or standard input ends
    /// without one.
    pub(crate) fn confirm(&self, question: &str) -> Result<(), Failure> {
        if self.yes {
            return Ok(());
        }
        if self.json || !self.interactive {
            return Err(
                Failure::new(exit::USAGE, "confirmation needed, but nobody can be asked")
                    .hint("rerun with -y to proceed without asking"),
            );
        }
        eprint!("{question} [Y/n] ");
        let _ = io::stderr().flush();
        let mut answer = String::new();
        let read = io::stdin()
            .lock()
            .read_line(&mut answer)
            .map_err(|e| Failure::new(exit::ABORTED, format!("no answer: {e}")))?;
        answer_is_yes(read, &answer)
            .then_some(())
            .ok_or_else(|| Failure::new(exit::ABORTED, "aborted"))
    }

    /// Whether standard error gets colors.
    pub(crate) fn colored_err(&self) -> bool {
        self.color_err
    }

    /// Writes a failure to standard error, or as a JSON error on standard
    /// output.
    pub(crate) fn failure(&self, failure: &Failure) {
        if let Some(report) = &failure.report {
            crate::report::emit(self, report);
        }
        if self.json {
            self.out(
                json!({
                    "event": "error",
                    "laft_json": LAFT_JSON,
                    "code": failure.code,
                    "exit": failure.exit,
                    "message": failure.message,
                    "detail": {"context": failure.context, "hints": failure.hints},
                })
                .to_string(),
            );
            return;
        }
        if failure.report.is_some() && failure.message.is_empty() {
            return;
        }
        let head = match &failure.code {
            Some(code) => format!("error[{code}]"),
            None => "error".into(),
        };
        let mut text = format!(
            "{}: {}\n",
            self.paint_err(Paint::BoldRed, &head),
            failure.message
        );
        for line in &failure.context {
            let _ = writeln!(text, "  {line}");
        }
        for hint in &failure.hints {
            let _ = writeln!(
                text,
                "  {}",
                self.paint_err(Paint::Dim, &format!("hint: {hint}"))
            );
        }
        let _ = io::stderr().lock().write_all(text.as_bytes());
    }
}

fn paint_if(on: bool, paint: Paint, text: &str) -> String {
    if on {
        format!("\x1b[{}m{text}\x1b[0m", paint.ansi())
    } else {
        text.to_owned()
    }
}

/// The size of the terminal on standard output, as columns and rows.
pub(crate) fn terminal_size() -> Option<(usize, usize)> {
    let size = rustix::termios::tcgetwinsize(io::stdout()).ok()?;
    (size.ws_col > 0 && size.ws_row > 0).then_some((size.ws_col.into(), size.ws_row.into()))
}

fn taller_than_screen(text: &str) -> bool {
    terminal_size().is_some_and(|(_, rows)| text.lines().count() >= rows)
}

/// Pipes `text` through `$LAFT_PAGER`, `$PAGER` or `less -FRX`, and returns
/// whether that worked.
fn page(text: &str) -> bool {
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    let command = env("LAFT_PAGER")
        .or_else(|| env("PAGER"))
        .unwrap_or_else(|| "less -FRX".into());
    let mut words = command.split_whitespace();
    let Some(program) = words.next() else {
        return false;
    };
    let Ok(mut child) = std::process::Command::new(program)
        .args(words)
        .stdin(std::process::Stdio::piped())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    child.wait().is_ok()
}

/// Aligns `rows` in columns under `header`, two spaces apart, and returns
/// the lines. The last column is not padded.
pub(crate) fn table(header: &[&str], rows: &[Vec<String>]) -> String {
    let columns = rows
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(0)
        .max(header.len());
    let mut widths = vec![0; columns];
    for (i, h) in header.iter().enumerate() {
        widths[i] = width(h);
    }
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(columns) {
            widths[i] = widths[i].max(width(cell));
        }
    }
    let mut out = String::new();
    let mut line = |cells: &mut dyn Iterator<Item = &str>| {
        let mut text = String::new();
        for (i, cell) in cells.enumerate() {
            if i + 1 == columns {
                text.push_str(cell);
            } else {
                text.push_str(cell);
                text.push_str(&" ".repeat(widths[i] - width(cell) + 2));
            }
        }
        out.push_str(text.trim_end());
        out.push('\n');
    };
    if !header.is_empty() {
        line(&mut header.iter().copied());
    }
    for row in rows {
        line(&mut row.iter().map(String::as_str));
    }
    out
}

/// The display width of `text`: its characters, without ANSI escapes.
pub(crate) fn width(text: &str) -> usize {
    let mut count = 0;
    let mut escape = false;
    for c in text.chars() {
        match (escape, c) {
            (false, '\x1b') => escape = true,
            (true, 'm') => escape = false,
            (true, _) => {}
            (false, _) => count += 1,
        }
    }
    count
}

/// `text` cut to `max` characters, ending with an ellipsis when cut.
pub(crate) fn truncate(text: &str, max: usize, ascii: bool) -> String {
    if width(text) <= max {
        return text.to_owned();
    }
    let ellipsis = if ascii { "..." } else { "\u{2026}" };
    let keep = max.saturating_sub(ellipsis.chars().count());
    let mut out: String = text.chars().take(keep).collect();
    out.push_str(ellipsis);
    out
}

/// A size in binary units with one decimal, such as `84.2 MiB`.
pub(crate) fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// A signed size difference, such as `+312.6 MiB`.
pub(crate) fn size_delta(bytes: i64) -> String {
    let sign = if bytes < 0 { "-" } else { "+" };
    format!("{sign}{}", size(bytes.unsigned_abs()))
}

/// A duration in its two largest units, such as `2h 14m`, `3.1s` or
/// `38 ms`.
pub(crate) fn duration(d: Duration) -> String {
    let secs = d.as_secs();
    match secs {
        0 => format!("{} ms", d.as_millis()),
        1..60 => format!("{:.1}s", d.as_secs_f64()),
        60..3600 => format!("{}m {}s", secs / 60, secs % 60),
        3600..86_400 => format!("{}h {}m", secs / 3600, secs / 60 % 60),
        _ => format!("{}d {}h", secs / 86_400, secs / 3600 % 24),
    }
}

/// How long ago `then` was, in its two largest units.
pub(crate) fn age(then: Timestamp, now: Timestamp) -> String {
    let secs = now.as_secs().saturating_sub(then.as_secs());
    if secs < 60 {
        return format!("{secs}s");
    }
    duration(Duration::from_secs(secs))
}

/// `time` as local time `YYYY-MM-DD HH:MM`.
pub(crate) fn local_time(time: Timestamp) -> String {
    let offset = crate::tz::offset_at(time.as_secs());
    let shifted = time.as_secs().saturating_add_signed(offset);
    let text = Timestamp::from_secs(shifted).to_string();
    format!("{} {}", &text[..10], &text[11..16])
}

/// Whether the `read` bytes of `answer` accept a `[Y/n]` question. An
/// empty line is yes, but the end of input is no.
fn answer_is_yes(read: usize, answer: &str) -> bool {
    read > 0
        && matches!(
            answer.trim().to_ascii_lowercase().as_str(),
            "" | "y" | "yes"
        )
}

#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;
