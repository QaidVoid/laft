use std::fmt;
use std::str::FromStr;

use crate::ParseError;

/// An option for a LUKS device that is unlocked at boot.
///
/// The system configuration and `laft-boot.json` name the options the same
/// way, so a configuration is checked against exactly the options the
/// initramfs understands.
///
/// ```
/// use laft_core::LuksOption;
///
/// let option: LuksOption = "no-read-workqueue".parse()?;
/// assert_eq!(option, LuksOption::NoReadWorkqueue);
/// assert!("trim".parse::<LuksOption>().is_err());
/// # Ok::<(), laft_core::ParseError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LuksOption {
    /// `discard`: pass discard requests to the encrypted device.
    Discard,
    /// `no-read-workqueue`: decrypt reads synchronously, without the read
    /// workqueue.
    NoReadWorkqueue,
    /// `no-write-workqueue`: encrypt writes synchronously, without the
    /// write workqueue.
    NoWriteWorkqueue,
}

impl LuksOption {
    /// The option's name in the configuration.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Discard => "discard",
            Self::NoReadWorkqueue => "no-read-workqueue",
            Self::NoWriteWorkqueue => "no-write-workqueue",
        }
    }
}

impl FromStr for LuksOption {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "discard" => Ok(Self::Discard),
            "no-read-workqueue" => Ok(Self::NoReadWorkqueue),
            "no-write-workqueue" => Ok(Self::NoWriteWorkqueue),
            _ => Err(ParseError::new(
                "LUKS option",
                s,
                "expected `discard`, `no-read-workqueue` or `no-write-workqueue`",
            )),
        }
    }
}

impl fmt::Display for LuksOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
