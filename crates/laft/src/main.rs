//! `laft`, the command that applies, switches and inspects a Laft system.
//!
//! ```text
//! laft plan
//! doas laft apply
//! laft gens list
//! doas laft rollback
//! laft --root /mnt apply -y
//! ```
//!
//! Commands that only read never take the transaction lock and never need
//! root. Commands that write take the lock and need root, except below a
//! root given with `--root`, which also means that nothing is activated on
//! the running system. Results go to standard output, progress, warnings
//! and errors to standard error. With `--json`, query commands print one
//! JSON document and long-running commands one JSON event per line.
//!
//! | Status | Meaning |
//! |---|---|
//! | 0 | success, including nothing to do |
//! | 1 | general failure |
//! | 2 | usage error, or a confirmation nobody can give |
//! | 3 | activated, but a service did not become ready |
//! | 4 | resolution error |
//! | 5 | network, index trust or package integrity error |
//! | 6 | path, alternatives or `/etc` conflict |
//! | 7 | the lock is busy and `--no-wait` was given |
//! | 8 | activated, but an activation step failed |
//! | 9 | `verify` or `doctor` found problems |
//! | 10 | declined at a confirmation |
//! | 11 | configuration error |
//! | 12 | `plan --exit-code` found changes |

#![forbid(unsafe_code)]

mod activate;
mod apply;
mod cli;
mod diff;
mod env;
mod etc;
mod explain;
mod failure;
mod gens;
mod lang;
mod live;
mod plan;
mod query;
mod repo;
mod report;
mod resolve;
mod tz;
mod ui;
mod verify;

use std::collections::BTreeSet;
use std::process::ExitCode;

use clap::Parser;
use laft_resolve::Upgrade;

use crate::cli::{Cli, Command, EtcCommand, GensCommand, RepoCommand, ViewCommand};
use crate::env::Env;
use crate::failure::{Failure, exit};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let env = Env::new(&cli.global);
    match run(&env, cli.command) {
        Ok(code) => ExitCode::from(code),
        Err(failure) => {
            env.ui.failure(&failure);
            ExitCode::from(failure.exit)
        }
    }
}

fn run(env: &Env, command: Command) -> Result<u8, Failure> {
    match command {
        Command::Plan(args) => apply::plan_command(env, &args),
        Command::Apply(args) => apply::apply(
            env,
            &apply::Request {
                command: "apply",
                upgrade: if args.upgrade {
                    Upgrade::All
                } else {
                    Upgrade::None
                },
                args,
                rebuild: BTreeSet::new(),
            },
        ),
        Command::Upgrade { names, apply } => {
            let upgrade = if names.is_empty() {
                Upgrade::All
            } else {
                Upgrade::Names(
                    names
                        .iter()
                        .map(|n| n.parse().map_err(|e| Failure::new(exit::USAGE, e)))
                        .collect::<Result<_, _>>()?,
                )
            };
            let mut args = apply;
            args.refresh = true;
            apply::apply(
                env,
                &apply::Request {
                    command: "upgrade",
                    args,
                    upgrade,
                    rebuild: BTreeSet::new(),
                },
            )
        }
        Command::View(ViewCommand::Build { names, apply }) => {
            let rebuild = if names.is_empty() {
                env.load(&|_| Ok(()))?.config.views.into_keys().collect()
            } else {
                names
                    .iter()
                    .map(|n| n.parse().map_err(|e| Failure::new(exit::USAGE, e)))
                    .collect::<Result<_, _>>()?
            };
            apply::apply(
                env,
                &apply::Request {
                    command: "view build",
                    args: apply,
                    upgrade: Upgrade::None,
                    rebuild,
                },
            )
        }
        Command::Switch { generation, boot } => gens::switch(env, generation, boot, "switch"),
        Command::Rollback { boot } => gens::rollback(env, boot),
        Command::Gens(GensCommand::List) => gens::list(env),
        Command::Gens(GensCommand::Show {
            generation,
            packages,
        }) => gens::show(env, generation, packages),
        Command::Gens(GensCommand::Diff { a, b, etc }) => gens::diff(env, a, b, etc),
        Command::Gens(GensCommand::Rm { generations, no_gc }) => gens::rm(env, &generations, no_gc),
        Command::Gc(args) => gens::gc(env, &args),
        Command::Verify(args) => verify::verify(env, &args),
        Command::Doctor => verify::doctor(env),
        Command::Etc(EtcCommand::Status) => etc::status(env),
        Command::Etc(EtcCommand::Diff { paths }) => etc::diff(env, &paths),
        Command::Etc(EtcCommand::Reset { paths }) => etc::reset(env, &paths),
        Command::Search {
            query,
            repo,
            all_versions,
        } => query::search(env, &query, repo.as_deref(), all_versions),
        Command::Info { pkg } => query::info(env, &pkg),
        Command::Files { pkg, long } => query::files(env, &pkg, long),
        Command::Owns { target, view } => query::owns(env, &target, view.as_deref()),
        Command::Why { name, view, all } => query::why(env, &name, view.as_deref(), all),
        Command::Repo(RepoCommand::Sync { names }) => repo::sync(env, &names),
        Command::Repo(RepoCommand::List) => repo::list(env),
        Command::Repo(RepoCommand::Reset { name }) => repo::reset(env, &name),
        Command::View(ViewCommand::List) => repo::view_list(env),
        Command::View(ViewCommand::Show { name }) => repo::view_show(env, &name),
        Command::Fmt {
            paths,
            check,
            stdin,
        } => lang::fmt(env, &paths, check, stdin),
        Command::Check { file, refresh } => lang::check(env, file.as_deref(), refresh),
        Command::Eval { attr, file, output } => {
            lang::eval(env, attr.as_deref(), file.as_deref(), output)
        }
        Command::Options { prefix, all } => lang::options(env, prefix.as_deref(), all),
        Command::Explain { code } => lang::explain(env, &code),
        _ => Err(Failure::new(exit::USAGE, "not available yet")),
    }
}
