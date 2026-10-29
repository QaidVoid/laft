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
mod env;
mod failure;
mod gens;
mod live;
mod plan;
mod report;
mod resolve;
mod tz;
mod ui;

use std::collections::BTreeSet;
use std::process::ExitCode;

use clap::Parser;
use laft_resolve::Upgrade;

use crate::cli::{Cli, Command, GensCommand, ViewCommand};
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
        Command::Gc(args) => gens::gc(env, &args),
        _ => Err(Failure::new(exit::USAGE, "not available yet")),
    }
}
