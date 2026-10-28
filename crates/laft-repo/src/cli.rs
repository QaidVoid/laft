//! The command line.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use laft_core::{Name, Timestamp, Variant, Version};
use laft_index::KeyId;
use laft_pkg::Arch;

use crate::sign::PASSWORD_ENV;

/// Builds, checks, signs and publishes Laft package repositories.
#[derive(Debug, Parser)]
#[command(name = "laft-repo", version)]
pub(crate) struct Cli {
    /// Use this time, in RFC 3339 UTC form, instead of the system clock as
    /// the signing time and for every age check.
    #[arg(long, global = true, value_name = "TIME")]
    pub(crate) now: Option<Timestamp>,
    /// The command.
    #[command(subcommand)]
    pub(crate) command: Command,
}

/// A `laft-repo` command.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Create a minisign key pair `PREFIX.key` and `PREFIX.pub`.
    Keygen {
        /// The path of the key files without extension.
        #[arg(long, value_name = "PREFIX")]
        out: PathBuf,
        /// Store the secret key without a password.
        #[arg(long)]
        unencrypted: bool,
        /// The environment variable that holds the password. Without it,
        /// the password is read from the terminal.
        #[arg(long, value_name = "VAR", default_value = PASSWORD_ENV)]
        password_env: String,
    },
    /// Add package files to a channel as a new index generation.
    ///
    /// Each PATH is a package file or a directory of build outputs, whose
    /// *.lpkg files are added and whose *.buildinfo.json files are stored
    /// as `buildinfo/<id>.json` for every output. The new index is checked
    /// before anything is written.
    Index {
        /// The channel.
        #[command(flatten)]
        channel: ChannelArgs,
        /// The repository name, required for a channel without an index.
        #[arg(long)]
        name: Option<Name>,
        /// Set the default member of an alternatives group.
        #[arg(long, value_name = "GROUP=NAME", value_parser = parse_pair::<Name>)]
        alternative: Vec<(Name, Name)>,
        /// Set the preferred provider of a capability, for example
        /// so:libjpeg.so.8=libjpeg-turbo.
        #[arg(long, value_name = "PROVIDE=NAME", value_parser = parse_pair::<String>)]
        provider: Vec<(String, Name)>,
        /// Check options.
        #[command(flatten)]
        check: CheckArgs,
        /// Signing options.
        #[command(flatten)]
        sign: SignArgs,
        /// Generation options.
        #[command(flatten)]
        generation: GenerationArgs,
        /// Package files and directories of build outputs.
        #[arg(value_name = "PATH")]
        inputs: Vec<PathBuf>,
    },
    /// Sign a channel's index with an online key, or keys.json with a root
    /// key.
    Sign {
        /// The repository directory.
        #[arg(long)]
        repo: PathBuf,
        /// The channel whose index to sign.
        #[arg(long, required_unless_present = "keys", conflicts_with = "keys")]
        channel: Option<Name>,
        /// The architecture of the index.
        #[arg(long, default_value = "x86_64-v3")]
        arch: Arch,
        /// Sign keys.json instead of an index.
        #[arg(long)]
        keys: bool,
        /// The secret key file.
        #[arg(long)]
        key: PathBuf,
        /// The environment variable that holds the key's password.
        #[arg(long, value_name = "VAR", default_value = PASSWORD_ENV)]
        password_env: String,
    },
    /// Run the repository-wide checks over a channel's index.
    ///
    /// The exit status is 1 when a check reports an error.
    Check {
        /// The channel.
        #[command(flatten)]
        channel: ChannelArgs,
        /// Check options.
        #[command(flatten)]
        check: CheckArgs,
        /// Print the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Withdraw a package from a channel with a reason, as a new
    /// generation.
    Yank {
        /// The channel.
        #[command(flatten)]
        channel: ChannelArgs,
        /// The package name.
        name: Name,
        /// The upstream version.
        version: Version,
        /// Only this revision.
        #[arg(long)]
        rev: Option<u32>,
        /// Only this variant.
        #[arg(long)]
        variant: Option<Variant>,
        /// Why it is withdrawn, one line shown to users.
        #[arg(long)]
        reason: String,
        /// Signing options.
        #[command(flatten)]
        sign: SignArgs,
        /// Generation options.
        #[command(flatten)]
        generation: GenerationArgs,
    },
    /// Copy the packages of one channel into another as a new generation,
    /// once the gates hold.
    Promote {
        /// The repository directory.
        #[arg(long)]
        repo: PathBuf,
        /// The channel to promote.
        #[arg(long, default_value = "testing")]
        from: Name,
        /// The channel to promote into.
        #[arg(long, default_value = "current")]
        to: Name,
        /// The architecture.
        #[arg(long, default_value = "x86_64-v3")]
        arch: Arch,
        /// Gate: the source generation was signed at least this long ago,
        /// for example 24h.
        #[arg(long, value_name = "DURATION", value_parser = parse_duration)]
        min_soak: Option<u64>,
        /// Gate: a report of `laft-repo check --json` on the source
        /// generation without errors.
        #[arg(long, value_name = "FILE")]
        check_report: Option<PathBuf>,
        /// Indexes of base repositories for the dependency checks.
        #[arg(long, value_name = "FILE")]
        with_index: Vec<PathBuf>,
        /// Signing options.
        #[command(flatten)]
        sign: SignArgs,
        /// Generation options.
        #[command(flatten)]
        generation: GenerationArgs,
    },
    /// Create or change keys.json, the list of online signing keys.
    Keys {
        /// The change.
        #[command(subcommand)]
        change: KeysCommand,
    },
    /// Copy a repository to a static target directory in an order that is
    /// safe for readers.
    Publish {
        /// The repository directory.
        repo: PathBuf,
        /// The target directory.
        target: PathBuf,
        /// Publish only this channel.
        #[arg(long)]
        channel: Option<Name>,
    },
}

/// A change to keys.json.
#[derive(Debug, Subcommand)]
pub(crate) enum KeysCommand {
    /// List an online key, creating keys.json when needed.
    Add {
        /// Common options.
        #[command(flatten)]
        common: KeysArgs,
        /// The repository name, required when there is no keys.json yet.
        #[arg(long)]
        name: Option<Name>,
        /// The minisign public key file of the online key.
        #[arg(long, value_name = "FILE")]
        public: PathBuf,
        /// The earliest index signing time the key is valid for. Defaults to
        /// now.
        #[arg(long, value_name = "TIME")]
        not_before: Option<Timestamp>,
        /// The latest index signing time the key is valid for.
        #[arg(long, value_name = "TIME")]
        not_after: Timestamp,
    },
    /// Stop listing an online key without revoking it.
    Remove {
        /// Common options.
        #[command(flatten)]
        common: KeysArgs,
        /// The key id, 16 lowercase hexadecimal digits.
        id: KeyId,
    },
    /// Revoke an online key for good.
    Revoke {
        /// Common options.
        #[command(flatten)]
        common: KeysArgs,
        /// The key id, 16 lowercase hexadecimal digits.
        id: KeyId,
        /// The last known good index generation.
        #[arg(long)]
        generation_floor: Option<u64>,
    },
}

/// The options every keys command takes.
#[derive(Debug, Args)]
pub(crate) struct KeysArgs {
    /// The repository directory.
    #[arg(long)]
    pub(crate) repo: PathBuf,
    /// The root secret key to sign keys.json with. Without it, keys.json
    /// stays unsigned until `laft-repo sign --keys`.
    #[arg(long)]
    pub(crate) key: Option<PathBuf>,
    /// The environment variable that holds the key's password.
    #[arg(long, value_name = "VAR", default_value = PASSWORD_ENV)]
    pub(crate) password_env: String,
}

/// Which channel of which repository.
#[derive(Debug, Args)]
pub(crate) struct ChannelArgs {
    /// The repository directory.
    #[arg(long)]
    pub(crate) repo: PathBuf,
    /// The channel, for example current or testing.
    #[arg(long)]
    pub(crate) channel: Name,
    /// The architecture.
    #[arg(long, default_value = "x86_64-v3")]
    pub(crate) arch: Arch,
}

/// Options of the repository checks.
#[derive(Debug, Args)]
pub(crate) struct CheckArgs {
    /// An index of a base repository that dependencies may resolve to.
    #[arg(long, value_name = "FILE")]
    pub(crate) with_index: Vec<PathBuf>,
    /// Allow packages from dirty or bootstrap builds.
    #[arg(long)]
    pub(crate) allow_bootstrap: bool,
}

/// How a new generation is signed.
#[derive(Debug, Args)]
pub(crate) struct SignArgs {
    /// The online secret key to sign the new generation with. Without it,
    /// the generation stays unsigned until `laft-repo sign`.
    #[arg(long)]
    pub(crate) key: Option<PathBuf>,
    /// The environment variable that holds the key's password.
    #[arg(long, value_name = "VAR", default_value = PASSWORD_ENV)]
    pub(crate) password_env: String,
}

/// How long a new generation is valid and what is kept.
#[derive(Debug, Args)]
pub(crate) struct GenerationArgs {
    /// How long the new index is valid, at most 30d.
    #[arg(long, value_name = "DURATION", default_value = "14d", value_parser = parse_duration)]
    pub(crate) expires_in: u64,
    /// How many snapshots of the channel to keep.
    #[arg(long, value_name = "N", default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..))]
    pub(crate) keep_snapshots: u64,
}

/// Parses a duration written as a number and a unit: `s`, `m`, `h` or `d`.
/// Returns seconds.
pub(crate) fn parse_duration(s: &str) -> Result<u64, String> {
    let invalid = || format!("{s:?} is not a duration such as 90s, 30m, 24h or 14d");
    let split = s.find(|c: char| !c.is_ascii_digit()).ok_or_else(invalid)?;
    let (number, unit) = s.split_at(split);
    let unit = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => return Err(invalid()),
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(unit))
        .ok_or_else(invalid)
}

/// Parses `<key>=<name>`.
fn parse_pair<K: std::str::FromStr>(s: &str) -> Result<(K, Name), String>
where
    K::Err: std::fmt::Display,
{
    let (key, value) = s
        .split_once('=')
        .ok_or_else(|| format!("{s:?} is not of the form KEY=NAME"))?;
    let key = key.parse().map_err(|e: K::Err| e.to_string())?;
    let value = value
        .parse()
        .map_err(|e: laft_core::ParseError| e.to_string())?;
    Ok((key, value))
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
