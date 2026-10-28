//! Messages to the kernel log, the boot log file and the console.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

/// The kernel log level of progress messages.
const INFO: u8 = 6;
/// The kernel log level of warnings.
const WARNING: u8 = 4;
/// The kernel log level of errors.
const ERROR: u8 = 3;

/// Where the boot reports what it does.
///
/// Every message goes to the kernel log as `laft-boot: <message>` with its
/// level, so that `quiet` hides progress but not problems, and to the boot
/// log file. Before the kernel log is open, messages go to the console.
/// Progress also goes to the console in debug mode.
#[derive(Debug, Default)]
pub(crate) struct Log {
    kmsg: Option<File>,
    file: Option<File>,
    debug: bool,
}

impl Log {
    /// Opens the kernel log device `kmsg` and the boot log file `file`. A
    /// target that cannot be opened is skipped.
    pub(crate) fn open(&mut self, kmsg: &Path, file: &Path) {
        self.kmsg = OpenOptions::new().write(true).open(kmsg).ok();
        self.file = OpenOptions::new().create(true).append(true).open(file).ok();
    }

    /// Sends progress messages to the console too when `debug` is set.
    pub(crate) fn set_debug(&mut self, debug: bool) {
        self.debug = debug;
    }

    /// Reports progress.
    pub(crate) fn info(&mut self, message: &str) {
        self.write(INFO, message);
    }

    /// Reports a problem the boot continues after.
    pub(crate) fn warn(&mut self, message: &str) {
        self.write(WARNING, message);
    }

    /// Reports a failure.
    pub(crate) fn error(&mut self, message: &str) {
        self.write(ERROR, message);
    }

    /// Records a failure that the caller prints on the console itself.
    pub(crate) fn record_error(&mut self, message: &str) {
        self.write_to(ERROR, message, false);
    }

    fn write(&mut self, level: u8, message: &str) {
        self.write_to(level, message, true);
    }

    fn write_to(&mut self, level: u8, message: &str, console_fallback: bool) {
        let prefix = match level {
            ERROR => "error: ",
            WARNING => "warning: ",
            _ => "",
        };
        for line in message.lines() {
            let to_console = match &mut self.kmsg {
                // The kernel log takes one record per write.
                Some(kmsg) => kmsg
                    .write_all(format!("<{level}>laft-boot: {line}\n").as_bytes())
                    .is_err(),
                None => true,
            };
            if (to_console && console_fallback) || (self.debug && level == INFO) {
                console(&format!("{prefix}{line}"));
            }
            if let Some(file) = &mut self.file {
                let _ = file.write_all(format!("{prefix}{line}\n").as_bytes());
            }
        }
    }
}

/// Prints `message` on the console, each line starting with `laft-boot: `.
pub(crate) fn console(message: &str) {
    let mut out = String::new();
    for line in message.lines() {
        out.push_str("laft-boot: ");
        out.push_str(line);
        out.push('\n');
    }
    let _ = std::io::stderr().write_all(out.as_bytes());
}
