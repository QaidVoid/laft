//! Checks of the configuration that need nothing but the configuration.

use crate::config::{EtcConfig, Security};
use crate::error::ConfigError;
use crate::{accounts, privileged, render};

/// Checks `config` and `security` with every rule that [`declare`],
/// [`render`] and [`assemble`] apply to the configuration alone, and
/// returns every problem found.
///
/// It reads nothing: ids are checked as if none had ever been allocated,
/// `source` files are not read, and owners are not resolved, because the
/// configuration language checks references to accounts. Assembly applies
/// the same rules again, together with those that need the store, the
/// recorded ids and the current account databases.
///
/// ```
/// use laft_activate::{EtcConfig, File, Security, check_config};
///
/// let mut config = EtcConfig::default();
/// config.files.insert("/etc/passwd".into(), File::default());
/// let errors = check_config(&config, &Security::default());
/// assert_eq!(errors[0].code(), Some("C0421"));
/// ```
///
/// [`declare`]: crate::declare
/// [`render`]: crate::render()
/// [`assemble`]: crate::assemble()
pub fn check_config(config: &EtcConfig, security: &Security) -> Vec<ConfigError> {
    let mut errors = accounts::check(&config.users);
    render::check(config, &mut errors);
    for (path, entry) in &security.privileged {
        if entry.enable {
            privileged::check_entry(path, entry, &mut errors);
        }
    }
    errors
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
