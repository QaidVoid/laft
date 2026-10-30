//! Why a command failed and the exit status that says so.

use std::fmt;

/// The exit statuses of `laft`.
pub(crate) mod exit {
    /// A general failure, or a check that failed.
    pub(crate) const FAILURE: u8 = 1;
    /// A usage error, or a confirmation nobody can give.
    pub(crate) const USAGE: u8 = 2;
    /// Activation completed, but a service failed to become ready.
    pub(crate) const SERVICES: u8 = 3;
    /// The packages of a view cannot be resolved.
    pub(crate) const RESOLVE: u8 = 4;
    /// A network, index trust or package integrity error.
    pub(crate) const NETWORK: u8 = 5;
    /// A path, alternatives or `/etc` conflict.
    pub(crate) const CONFLICT: u8 = 6;
    /// The lock is held and `--no-wait` was given.
    pub(crate) const BUSY: u8 = 7;
    /// The generation is active but an activation step failed.
    pub(crate) const PARTIAL: u8 = 8;
    /// `verify` or `doctor` found problems.
    pub(crate) const PROBLEMS: u8 = 9;
    /// The user declined a confirmation.
    pub(crate) const ABORTED: u8 = 10;
    /// The configuration is invalid.
    pub(crate) const CONFIG: u8 = 11;
    /// `plan --exit-code` found changes.
    pub(crate) const CHANGES: u8 = 12;
}

/// A failed command: the exit status, an optional diagnostic code, a
/// one-line message, indented context lines and hints.
#[derive(Debug)]
pub(crate) struct Failure {
    /// The exit status.
    pub(crate) exit: u8,
    /// The diagnostic code, such as `S003`.
    pub(crate) code: Option<String>,
    /// What went wrong, in one line.
    pub(crate) message: String,
    /// More lines about it.
    pub(crate) context: Vec<String>,
    /// What the user can do.
    pub(crate) hints: Vec<String>,
    /// Configuration diagnostics, rendered with their source.
    pub(crate) report: Option<Box<laft_config::Report>>,
}

impl Failure {
    /// A failure with status `exit` and `message`. A message of several
    /// lines keeps the first as the message and the others as context.
    pub(crate) fn new(exit: u8, message: impl fmt::Display) -> Self {
        let text = message.to_string();
        let mut lines = text.lines();
        let first = lines.next().unwrap_or_default().to_owned();
        Self {
            exit,
            code: None,
            message: first,
            context: lines.map(|l| l.trim_end().to_owned()).collect(),
            hints: Vec::new(),
            report: None,
        }
    }

    /// A general failure with `message`.
    pub(crate) fn general(message: impl fmt::Display) -> Self {
        Self::new(exit::FAILURE, message)
    }

    /// Adds a diagnostic code.
    pub(crate) fn code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    /// Adds a context line.
    pub(crate) fn context(mut self, line: impl Into<String>) -> Self {
        self.context.push(line.into());
        self
    }

    /// Adds a hint.
    pub(crate) fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hints.push(hint.into());
        self
    }

    /// The failure of an invalid configuration, whose diagnostics are
    /// rendered on their own.
    pub(crate) fn config(report: laft_config::Report) -> Self {
        let errors = report
            .iter()
            .filter(|d| d.diagnostic.severity == stav::Severity::Error)
            .count();
        let mut failure = Self::new(
            exit::CONFIG,
            format!(
                "the configuration has {errors} error{}",
                if errors == 1 { "" } else { "s" }
            ),
        );
        failure.code = report
            .iter()
            .find(|d| d.diagnostic.severity == stav::Severity::Error)
            .map(|d| d.diagnostic.code.to_owned());
        failure.report = Some(Box::new(report));
        failure
    }

    /// The failure of a command that writes, run without the right to.
    pub(crate) fn needs_root(command: &str) -> Self {
        Self::general(format!(
            "laft {command} needs root (try: doas laft {command})"
        ))
    }
}

impl From<laft_store::Error> for Failure {
    fn from(error: laft_store::Error) -> Self {
        use laft_store::Error;
        let exit = match &error {
            Error::Conflicts { .. } | Error::DuplicatePackage { .. } => exit::CONFLICT,
            Error::IdMismatch { .. } | Error::Package(_) => exit::NETWORK,
            _ => exit::FAILURE,
        };
        let mut failure = Self::new(exit, &error);
        if let Error::Conflicts { conflicts, .. } = &error {
            for conflict in conflicts {
                failure =
                    failure.context(format!("{}  {}", conflict.path, conflict.owners.join(", ")));
            }
            failure =
                failure.hint("these packages share no alternatives group, so remove one of them");
        }
        if let Error::LinkLimit { .. } = &error {
            failure = failure.hint("run `laft gc` or lower gc.keep_generations");
        }
        if let Error::Pending { .. } = &error {
            failure = failure.hint("run `laft apply` or `laft gc` as root to recover it");
        }
        failure
    }
}

impl From<laft_activate::Error> for Failure {
    fn from(error: laft_activate::Error) -> Self {
        use laft_activate::Error;
        match error {
            Error::Store(e) => e.into(),
            Error::Conflicts(conflicts) => {
                let mut failure = Self::new(
                    exit::CONFLICT,
                    format!(
                        "{} path{} in /etc {} not managed by laft",
                        conflicts.len(),
                        if conflicts.len() == 1 { "" } else { "s" },
                        if conflicts.len() == 1 { "is" } else { "are" }
                    ),
                );
                for conflict in &conflicts {
                    let text = conflict.to_string();
                    failure = failure.context(text.lines().next().unwrap_or_default());
                }
                let first = conflicts
                    .first()
                    .map_or_else(|| "<path>".to_owned(), |c| format!("/etc/{}", c.path));
                failure.hint(format!(
                    "run `laft apply --adopt {first}` to move it aside as {first}.laft-orig"
                ))
            }
            other => Self::general(other),
        }
    }
}

impl From<laft_fetch::Error> for Failure {
    fn from(error: laft_fetch::Error) -> Self {
        let mut failure = Self::new(exit::NETWORK, &error);
        if matches!(
            error.kind(),
            laft_fetch::ErrorKind::Offline | laft_fetch::ErrorKind::NotCached { .. }
        ) {
            failure = failure.hint("run without --offline to download it");
        }
        failure
    }
}

impl From<laft_index::Error> for Failure {
    fn from(error: laft_index::Error) -> Self {
        Self::new(exit::NETWORK, error)
    }
}

impl From<laft_resolve::Error> for Failure {
    fn from(error: laft_resolve::Error) -> Self {
        use laft_resolve::ErrorKind;
        let exit = match *error.kind {
            ErrorKind::AlternativeAbsent { .. } | ErrorKind::AlternativeAmbiguous { .. } => {
                exit::CONFLICT
            }
            _ => exit::RESOLVE,
        };
        let code = error.code();
        let mut failure = Self::new(exit, &error);
        if let Some(code) = code {
            failure = failure.code(code);
        }
        failure
    }
}

impl From<laft_svc::Error> for Failure {
    fn from(error: laft_svc::Error) -> Self {
        Self::general(error)
    }
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        Self::general(error)
    }
}

#[cfg(test)]
#[path = "failure_tests.rs"]
mod tests;
