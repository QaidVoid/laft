//! `laft-repo`, the build-side tool that maintains Laft package
//! repositories.
//!
//! A repository is a directory of static files: package files in
//! `pool/<id>.lpkg`, their buildinfo in `buildinfo/<id>.json`, one signed
//! index per channel and architecture in `channels/<channel>/<arch>/` with
//! immutable snapshots of its recent generations, and optionally the
//! root-signed list of online keys in `keys.json`.
//!
//! ```text
//! laft-repo keygen --out keys/online --unencrypted
//! laft-repo index --repo repo --name core --channel testing --key keys/online.key out/
//! laft-repo check --repo repo --channel testing --json > report.json
//! laft-repo promote --repo repo --min-soak 24h --check-report report.json --key keys/online.key
//! laft-repo yank --repo repo --channel current zlib 1.3.1 --reason "breaks gzip" --key keys/online.key
//! laft-repo keys add --repo repo --public keys/next.pub --not-after 2027-10-01T00:00:00Z --key keys/root.key
//! laft-repo publish repo /srv/www/laft
//! ```
//!
//! Commands that write a new index generation sign it when `--key` is
//! given, and leave it unsigned for `laft-repo sign` otherwise. Encrypted
//! secret keys are decrypted with the password in the environment
//! variable `LAFT_SIGNING_KEY_PASSWORD`, or the one `--password-env`
//! names, and with a password typed at the terminal when it is unset.
//!
//! The exit status is 0 on success, 1 on failure or when a check reports
//! an error, and 2 for a usage error.

#![forbid(unsafe_code)]

mod check;
mod cli;
mod error;
mod fsx;
mod generation;
mod index;
mod keys;
mod layout;
mod package;
mod promote;
mod publish;
mod sign;
mod yank;

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use laft_core::Timestamp;
use laft_index::Index;

use crate::check::Report;
use crate::cli::{Cli, Command, GenerationArgs, KeysCommand, SignArgs};
use crate::error::{Error, Result, io};
use crate::generation::Options;
use crate::index::Outcome;
use crate::layout::{Channel, Repository};
use crate::sign::Signer;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let now = cli.now.unwrap_or_else(Timestamp::now);
    match run(cli.command, now) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("laft-repo: {e}");
            ExitCode::from(1)
        }
    }
}

/// Runs one command. Returns whether it succeeded without check errors.
fn run(command: Command, now: Timestamp) -> Result<bool> {
    let mut out = io::stdout().lock();
    let say = |out: &mut io::StdoutLock<'_>, text: String| {
        writeln!(out, "{text}").map_err(|e| Error::new(format!("stdout: {e}")))
    };
    match command {
        Command::Keygen {
            out: prefix,
            unencrypted,
            password_env,
        } => {
            let password = std::env::var(&password_env).ok();
            let id = sign::keygen(&prefix, !unencrypted, password)?;
            say(&mut out, format!("created key {id}"))?;
            Ok(true)
        }
        Command::Index {
            channel,
            name,
            alternative,
            provider,
            check,
            sign,
            generation,
            inputs,
        } => {
            let repo = Repository::new(&channel.repo);
            let channel = Channel {
                name: channel.channel,
                arch: channel.arch,
            };
            let signer = signer(&sign)?;
            let bases = read_indexes(&check.with_index)?;
            let outcome = index::run(&index::Request {
                repo: &repo,
                channel: &channel,
                name: name.as_ref(),
                inputs: &inputs,
                alternatives: &alternative,
                providers: &provider,
                bases: &bases,
                allow_bootstrap: check.allow_bootstrap,
                signer: signer.as_ref(),
                options: options(&generation, now),
            })?;
            match outcome {
                Outcome::Written {
                    index,
                    added,
                    report,
                } => {
                    print_findings(&mut out, &report)?;
                    say(
                        &mut out,
                        format!(
                            "{channel} generation {}: {} packages, {added} new{}",
                            index.generation,
                            index.packages.len(),
                            unsigned(signer.is_none())
                        ),
                    )?;
                    Ok(true)
                }
                Outcome::Refused(report) => refused(&mut out, &report),
            }
        }
        Command::Sign {
            repo,
            channel,
            arch,
            keys,
            key,
            password_env,
        } => {
            let repo = Repository::new(repo);
            let signer = Signer::load(&key, std::env::var(&password_env).ok())?;
            if keys {
                let keys = keys::sign(&repo, &signer)?;
                say(
                    &mut out,
                    format!(
                        "signed keys.json version {} with key {}",
                        keys.version,
                        signer.id()
                    ),
                )?;
            } else if let Some(name) = channel {
                let channel = Channel { name, arch };
                let comment = generation::sign(&repo, &channel, &signer)?;
                say(
                    &mut out,
                    format!("signed \"{comment}\" with key {}", signer.id()),
                )?;
            }
            Ok(true)
        }
        Command::Check {
            channel,
            check,
            json,
        } => {
            let repo = Repository::new(&channel.repo);
            let channel = Channel {
                name: channel.channel,
                arch: channel.arch,
            };
            let report = check_channel(&repo, &channel, &check)?;
            if json {
                serde_json::to_writer(&mut out, &report)
                    .map_err(|e| Error::new(format!("stdout: {e}")))?;
                say(&mut out, String::new())?;
            } else {
                report
                    .write_text(&mut out)
                    .map_err(|e| Error::new(format!("stdout: {e}")))?;
            }
            Ok(report.errors == 0)
        }
        Command::Yank {
            channel,
            name,
            version,
            rev,
            variant,
            reason,
            sign,
            generation,
        } => {
            let repo = Repository::new(&channel.repo);
            let channel = Channel {
                name: channel.channel,
                arch: channel.arch,
            };
            let signer = signer(&sign)?;
            let request = yank::Request {
                name: &name,
                version: &version,
                rev,
                variant,
                reason: &reason,
            };
            let options = options(&generation, now);
            let (index, yanks) = yank::run(&repo, &channel, &request, signer.as_ref(), &options)?;
            for yank in &yanks {
                say(
                    &mut out,
                    format!(
                        "yanked {} {}-r{} ({})",
                        yank.name, yank.version, yank.rev, yank.id
                    ),
                )?;
            }
            say(
                &mut out,
                format!(
                    "{channel} generation {}{}",
                    index.generation,
                    unsigned(signer.is_none())
                ),
            )?;
            Ok(true)
        }
        Command::Promote {
            repo,
            from,
            to,
            arch,
            min_soak,
            check_report,
            with_index,
            sign,
            generation,
        } => {
            let repo = Repository::new(repo);
            let from = Channel { name: from, arch };
            let to = Channel { name: to, arch };
            let signer = signer(&sign)?;
            let bases = read_indexes(&with_index)?;
            let gates = promote::Gates {
                min_soak,
                check_report: check_report.as_deref(),
            };
            let options = options(&generation, now);
            let (source, outcome) =
                promote::run(&repo, &from, &to, &gates, &bases, signer.as_ref(), &options)?;
            match outcome {
                Outcome::Written { index, report, .. } => {
                    print_findings(&mut out, &report)?;
                    say(
                        &mut out,
                        format!(
                            "promoted {from} generation {source} to {to} generation {}{}",
                            index.generation,
                            unsigned(signer.is_none())
                        ),
                    )?;
                    Ok(true)
                }
                Outcome::Refused(report) => refused(&mut out, &report),
            }
        }
        Command::Keys { change } => {
            let (common, name, change) = match change {
                KeysCommand::Add {
                    common,
                    name,
                    public,
                    not_before,
                    not_after,
                } => {
                    let change = keys::Change::Add {
                        public: sign::read_public(&public)?,
                        not_before: not_before.unwrap_or(now),
                        not_after,
                    };
                    (common, name, change)
                }
                KeysCommand::Remove { common, id } => (common, None, keys::Change::Remove(id)),
                KeysCommand::Revoke {
                    common,
                    id,
                    generation_floor,
                } => (
                    common,
                    None,
                    keys::Change::Revoke {
                        id,
                        generation_floor,
                    },
                ),
            };
            let repo = Repository::new(&common.repo);
            let signer = signer(&SignArgs {
                key: common.key,
                password_env: common.password_env,
            })?;
            let keys = keys::run(&repo, name.as_ref(), change, signer.as_ref())?;
            say(
                &mut out,
                format!(
                    "keys.json version {}: {} keys, {} revoked{}",
                    keys.version,
                    keys.keys.len(),
                    keys.revoked.len(),
                    unsigned(signer.is_none())
                ),
            )?;
            Ok(true)
        }
        Command::Publish {
            repo,
            target,
            channel,
        } => {
            let repo = Repository::new(repo);
            let summary = publish::run(&repo, &target, channel.as_ref())?;
            for (channel, generation) in &summary.channels {
                say(
                    &mut out,
                    format!("published {channel} generation {generation}"),
                )?;
            }
            say(
                &mut out,
                format!(
                    "{} files copied, {} removed",
                    summary.copied, summary.removed
                ),
            )?;
            Ok(true)
        }
    }
}

/// Checks the entry point of `channel` against the snapshot of the
/// generation before it and the repository's other channels.
fn check_channel(repo: &Repository, channel: &Channel, args: &cli::CheckArgs) -> Result<Report> {
    let index = repo
        .read_index(channel)?
        .ok_or_else(|| Error::new(format!("channel {channel} has no index")))?;
    let previous = repo
        .snapshots(channel)?
        .into_iter()
        .rfind(|g| *g < index.generation);
    let published = match previous {
        Some(generation) => Some(read_index(&repo.path(&channel.snapshot(generation)))?),
        None => None,
    };
    let bases = read_indexes(&args.with_index)?;
    let others = repo.other_indexes(channel)?;
    Ok(check::check(&check::Input {
        repo,
        index: &index,
        staged: &Default::default(),
        staged_buildinfo: &Default::default(),
        published: published.as_ref(),
        others: &others,
        bases: &bases,
        allow_bootstrap: args.allow_bootstrap,
    }))
}

fn read_index(path: &Path) -> Result<Index> {
    let bytes = std::fs::read(path).map_err(io(path))?;
    Index::decode(&bytes).map_err(|e| Error::new(format!("{}: {e}", path.display())))
}

fn read_indexes(paths: &[PathBuf]) -> Result<Vec<Index>> {
    paths.iter().map(|p| read_index(p)).collect()
}

fn signer(args: &SignArgs) -> Result<Option<Signer>> {
    args.key
        .as_ref()
        .map(|key| Signer::load(key, std::env::var(&args.password_env).ok()))
        .transpose()
}

fn options(args: &GenerationArgs, now: Timestamp) -> Options {
    Options {
        now,
        expires_in: args.expires_in,
        keep: usize::try_from(args.keep_snapshots).unwrap_or(usize::MAX),
    }
}

fn unsigned(unsigned: bool) -> &'static str {
    if unsigned { ", unsigned" } else { "" }
}

fn print_findings(out: &mut impl Write, report: &Report) -> Result<()> {
    if report.findings.is_empty() {
        return Ok(());
    }
    report
        .write_text(out)
        .map_err(|e| Error::new(format!("stdout: {e}")))
}

fn refused(out: &mut impl Write, report: &Report) -> Result<bool> {
    print_findings(out, report)?;
    Err(Error::new(format!(
        "the checks found {} errors, nothing was written",
        report.errors
    )))
}
