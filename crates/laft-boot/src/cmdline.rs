//! The `laft.*` parameters of the kernel command line.

/// The `laft.*` parameters that steer the boot.
///
/// When a parameter is given more than once, the last one wins.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Cmdline {
    /// The generation to boot, from `laft.gen=<N>`.
    pub(crate) generation: Option<u64>,
    /// Whether `laft.break=premount` asks for a shell before loading modules.
    pub(crate) break_premount: bool,
    /// Whether `laft.break=preswitch` asks for a shell before the switch to
    /// the new root.
    pub(crate) break_preswitch: bool,
    /// The program to execute instead of the generation's init, from
    /// `laft.init=<path>`.
    pub(crate) init: Option<String>,
    /// Whether `laft.notries` leaves the boot counting state alone.
    pub(crate) notries: bool,
    /// The seconds to wait for boot devices, from `laft.rootwait=<seconds>`.
    pub(crate) rootwait: Option<u64>,
    /// Whether `laft.debug` sends progress messages to the console.
    pub(crate) debug: bool,
    /// Problems with `laft.*` parameters that were ignored.
    pub(crate) warnings: Vec<String>,
}

impl Cmdline {
    /// Collects the `laft.*` parameters of the command line `text`, the
    /// content of `/proc/cmdline`. Parameters of other components are
    /// skipped. Unknown or malformed `laft.*` parameters are ignored and
    /// reported in [`Self::warnings`].
    pub(crate) fn parse(text: &str) -> Self {
        let mut cmdline = Self::default();
        for param in split(text) {
            if param.starts_with("laft.") {
                cmdline.apply(&param);
            }
        }
        cmdline
    }

    fn apply(&mut self, param: &str) {
        let (key, value) = match param.split_once('=') {
            Some((key, value)) => (key, Some(value)),
            None => (param, None),
        };
        match (key, value) {
            ("laft.gen", Some(value)) => match number(value) {
                Some(n) => self.generation = Some(n),
                None => self.ignore(param, "the generation is not a number"),
            },
            ("laft.break", Some("premount")) => self.break_premount = true,
            ("laft.break", Some("preswitch")) => self.break_preswitch = true,
            ("laft.break", Some(_)) => {
                self.ignore(param, "the break point is not premount or preswitch");
            }
            ("laft.init", Some(value)) if value.starts_with('/') => {
                self.init = Some(value.to_owned());
            }
            ("laft.init", Some(_)) => self.ignore(param, "the init is not an absolute path"),
            ("laft.notries", None) => self.notries = true,
            ("laft.rootwait", Some(value)) => match number(value) {
                Some(secs) => self.rootwait = Some(secs),
                None => self.ignore(param, "the wait is not a number of seconds"),
            },
            ("laft.debug", None) => self.debug = true,
            _ => self.ignore(param, "unknown parameter"),
        }
    }

    fn ignore(&mut self, param: &str, reason: &str) {
        self.warnings
            .push(format!("ignoring kernel parameter {param:?}: {reason}"));
    }
}

/// Splits a command line into parameters the way the kernel does.
///
/// Parameters are separated by ASCII whitespace outside double quotes, and
/// every double quote switches quoting on or off. Of the quotes, only these
/// are then removed: one that starts the parameter, one that starts its
/// value right after the first `=` that is not the first character, and
/// one that ends the parameter when either of those was removed. Every
/// other quote stays part of the parameter.
fn split(text: &str) -> Vec<String> {
    let mut params = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in text.chars() {
        if is_space(c) && !quoted {
            if !current.is_empty() {
                params.push(unquote(&std::mem::take(&mut current)));
            }
            continue;
        }
        if c == '"' {
            quoted = !quoted;
        }
        current.push(c);
    }
    if !current.is_empty() {
        params.push(unquote(&current));
    }
    params
}

/// Whitespace as the kernel's `isspace` sees it among ASCII characters.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r')
}

/// Removes the quotes of one parameter that the kernel removes.
fn unquote(raw: &str) -> String {
    let (body, quoted) = match raw.strip_prefix('"') {
        Some(body) => (body, true),
        None => (raw, false),
    };
    let equals = body.char_indices().skip(1).find(|&(_, c)| c == '=');
    let mut end = body.len();
    let mut value = None;
    if let Some((at, _)) = equals {
        let mut start = at + 1;
        if body[start..].starts_with('"') {
            start += 1;
            if body.ends_with('"') {
                end -= 1;
            }
        }
        value = Some((at, start));
    }
    if quoted && end == body.len() && body.ends_with('"') {
        end -= 1;
    }
    match value {
        Some((at, start)) => format!("{}={}", &body[..at], &body[start.min(end)..end]),
        None => body[..end].to_owned(),
    }
}

/// A decimal number made of digits only.
fn number(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

#[cfg(test)]
#[path = "cmdline_tests.rs"]
mod tests;
