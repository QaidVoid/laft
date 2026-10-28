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

/// Splits a command line on whitespace outside double quotes and drops the
/// quotes, the way the kernel reads its parameters.
fn split(text: &str) -> Vec<String> {
    let mut params = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_ascii_whitespace() && !quoted => {
                if !current.is_empty() {
                    params.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        params.push(current);
    }
    params
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
