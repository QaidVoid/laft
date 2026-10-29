//! The command line: global options and every command.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Applies, switches and inspects a Laft system.
#[derive(Debug, Parser)]
#[command(name = "laft", version, propagate_version = true)]
pub(crate) struct Cli {
    /// Global options.
    #[command(flatten)]
    pub(crate) global: Global,
    /// The command.
    #[command(subcommand)]
    pub(crate) command: Command,
}

/// Options every command accepts.
#[derive(Debug, Clone, Args)]
pub(crate) struct Global {
    /// The configuration entry point [default: $LAFT_CONFIG, else
    /// the system.stav in etc/laft of the root].
    #[arg(long, global = true, value_name = "PATH")]
    pub(crate) config: Option<PathBuf>,
    /// Operate on a Laft system mounted at DIR. Nothing is activated live.
    #[arg(long, global = true, value_name = "DIR")]
    pub(crate) root: Option<PathBuf>,
    /// Print machine readable JSON.
    #[arg(long, global = true)]
    pub(crate) json: bool,
    /// When to color human output [default: $LAFT_COLOR, else auto].
    #[arg(long, global = true, value_name = "WHEN")]
    pub(crate) color: Option<ColorChoice>,
    /// Use ASCII symbols instead of Unicode.
    #[arg(long, global = true)]
    pub(crate) ascii: bool,
    /// Print more detail. Repeat for even more.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub(crate) verbose: u8,
    /// Print only errors and the final result.
    #[arg(short, long, global = true)]
    pub(crate) quiet: bool,
    /// Assume yes to confirmations.
    #[arg(short = 'y', long = "yes", global = true)]
    pub(crate) yes: bool,
    /// Never use the network.
    #[arg(long, global = true)]
    pub(crate) offline: bool,
    /// Fail with status 7 instead of waiting for the lock.
    #[arg(long, global = true)]
    pub(crate) no_wait: bool,
    /// Never page long output.
    #[arg(long, global = true)]
    pub(crate) no_pager: bool,
    /// How configuration diagnostics are written to standard error.
    #[arg(long, global = true, value_name = "FORMAT", default_value = "human")]
    pub(crate) message_format: MessageFormat,
}

/// When human output is colored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ColorChoice {
    /// When standard error or output is a terminal.
    Auto,
    /// Always.
    Always,
    /// Never.
    Never,
}

/// How configuration diagnostics are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum MessageFormat {
    /// Source excerpts with the spans underlined.
    Human,
    /// One JSON object per diagnostic and line.
    Json,
}

/// A `laft` command.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Show what `laft apply` would change, without changing anything.
    Plan(PlanArgs),
    /// Build a generation from the configuration and activate it.
    Apply(ApplyArgs),
    /// Apply with newer versions of the named packages, or of every
    /// package.
    Upgrade {
        /// Packages to upgrade. Every package when none is named.
        names: Vec<String>,
        /// Apply options.
        #[command(flatten)]
        apply: ApplyArgs,
    },
    /// Activate an existing generation and make it the default.
    Switch {
        /// The generation.
        generation: u64,
        /// Only make it the default for the next boot.
        #[arg(long)]
        boot: bool,
    },
    /// Switch to the parent of the active generation.
    Rollback {
        /// Only make it the default for the next boot.
        #[arg(long)]
        boot: bool,
    },
    /// Install Laft onto a target mounted at --root.
    Install {
        /// Write a configuration and hardware.stav for the target instead.
        #[arg(long)]
        generate_config: bool,
    },
    /// Reconcile /etc and accounts with the booted generation.
    Activate {
        /// Run as part of the boot sequence.
        #[arg(long, required = true)]
        boot: bool,
    },
    /// Generations.
    #[command(subcommand)]
    Gens(GensCommand),
    /// Boot entries.
    #[command(subcommand)]
    Boot(BootCommand),
    /// Delete generations, views and store entries nothing needs.
    Gc(GcArgs),
    /// Verify store entries and generations.
    Verify(VerifyArgs),
    /// Run health checks.
    Doctor,
    /// The managed files of /etc.
    #[command(subcommand)]
    Etc(EtcCommand),
    /// Enter a Laft system mounted at --root from a rescue environment.
    Rescue(RescueArgs),
    /// Enter a Laft system mounted at DIR from a rescue environment.
    Chroot {
        /// The mounted system.
        dir: PathBuf,
        /// Rescue options.
        #[command(flatten)]
        rescue: RescueArgs,
    },
    /// Search packages by name and summary.
    Search {
        /// The text to look for.
        query: String,
        /// Search only this repository.
        #[arg(long)]
        repo: Option<String>,
        /// List every version instead of the newest.
        #[arg(long)]
        all_versions: bool,
    },
    /// Show a package.
    Info {
        /// The package reference.
        pkg: String,
    },
    /// List the files of a package.
    Files {
        /// The package reference.
        pkg: String,
        /// Show type, mode, owner and size.
        #[arg(short = 'l')]
        long: bool,
    },
    /// Find what provides a path or command.
    Owns {
        /// An absolute path, or a command in /usr/bin.
        target: String,
        /// Look in this view instead of the system view.
        #[arg(long)]
        view: Option<String>,
    },
    /// Explain why a package is in a view.
    Why {
        /// The package name.
        name: String,
        /// The view.
        #[arg(long)]
        view: Option<String>,
        /// Explain it for every view.
        #[arg(long)]
        all: bool,
    },
    /// Package repositories.
    #[command(subcommand)]
    Repo(RepoCommand),
    /// Views besides the system view.
    #[command(subcommand)]
    View(ViewCommand),
    /// Run a command inside a view.
    Enter {
        /// The view.
        view: String,
        /// The command and its arguments. The login shell by default.
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Services.
    #[command(subcommand)]
    Svc(SvcCommand),
    /// Format configuration files.
    Fmt {
        /// Files to format. Every .stav file of the configuration by
        /// default.
        paths: Vec<PathBuf>,
        /// Change nothing and fail when a file would change.
        #[arg(long)]
        check: bool,
        /// Format standard input to standard output.
        #[arg(long, conflicts_with = "paths")]
        stdin: bool,
    },
    /// Check the configuration, or one file of it.
    Check {
        /// A file to check on its own, without resolving packages.
        file: Option<PathBuf>,
        /// Sync indexes even if they are fresh.
        #[arg(long)]
        refresh: bool,
    },
    /// Print the evaluated configuration.
    Eval {
        /// The attribute path to print, such as services.sshd.exec.
        attr: Option<String>,
        /// Evaluate this file instead of the configuration root.
        #[arg(long)]
        file: Option<PathBuf>,
        /// The output format.
        #[arg(short = 'o', default_value = "json")]
        output: EvalFormat,
    },
    /// List configuration options.
    Options {
        /// Only options whose path starts with this.
        prefix: Option<String>,
        /// Print whole doc comments.
        #[arg(long)]
        all: bool,
    },
    /// Explain a diagnostic code.
    Explain {
        /// The code, such as C0901 or R001.
        code: String,
    },
    /// Generate hardware.stav for this machine or the target at --root.
    Hardware {
        /// Write etc/laft/hardware.stav below the root instead of printing it.
        #[arg(long)]
        write: bool,
    },
    /// Print a shell completion script.
    Completions {
        /// The shell.
        shell: Shell,
    },
    /// Commands for Laft's own processes.
    #[command(subcommand, hide = true)]
    Internal(InternalCommand),
}

/// Options of `laft plan`.
#[derive(Debug, Args, Clone, Default)]
pub(crate) struct PlanArgs {
    /// Ignore the installed versions and take the newest ones.
    #[arg(long)]
    pub(crate) upgrade: bool,
    /// Sync indexes even if they are fresh.
    #[arg(long)]
    pub(crate) refresh: bool,
    /// Include unified diffs of changed /etc files.
    #[arg(long)]
    pub(crate) diff: bool,
    /// Move this unmanaged /etc file aside instead of failing.
    #[arg(long, value_name = "PATH")]
    pub(crate) adopt: Vec<String>,
    /// Exit with status 12 when the plan contains changes.
    #[arg(long)]
    pub(crate) exit_code: bool,
}

/// Options of `laft apply`.
#[derive(Debug, Args, Clone, Default)]
pub(crate) struct ApplyArgs {
    /// Only show the plan, as `laft plan` does.
    #[arg(long, conflicts_with_all = ["build_only", "boot"])]
    pub(crate) dry_run: bool,
    /// Build the generation without activating it or making it the default.
    #[arg(long, conflicts_with = "boot")]
    pub(crate) build_only: bool,
    /// Build the generation and make it the default for the next boot.
    #[arg(long)]
    pub(crate) boot: bool,
    /// Ignore the installed versions and take the newest ones.
    #[arg(long)]
    pub(crate) upgrade: bool,
    /// Sync indexes even if they are fresh.
    #[arg(long)]
    pub(crate) refresh: bool,
    /// Include unified diffs of changed /etc files.
    #[arg(long)]
    pub(crate) diff: bool,
    /// Store a message with the generation.
    #[arg(short, long)]
    pub(crate) message: Option<String>,
    /// Move this unmanaged /etc file aside instead of failing.
    #[arg(long, value_name = "PATH")]
    pub(crate) adopt: Vec<String>,
    /// Build a generation even when nothing changed.
    #[arg(long)]
    pub(crate) force: bool,
    /// Skip the automatic garbage collection.
    #[arg(long)]
    pub(crate) no_gc: bool,
}

/// `laft gens` commands.
#[derive(Debug, Subcommand)]
pub(crate) enum GensCommand {
    /// List the generations.
    List,
    /// Show a generation.
    Show {
        /// The generation.
        generation: u64,
        /// List every package.
        #[arg(long)]
        packages: bool,
    },
    /// Compare two generations.
    Diff {
        /// The older generation.
        a: u64,
        /// The newer generation [default: the active one].
        b: Option<u64>,
        /// Add unified diffs of changed /etc files.
        #[arg(long)]
        etc: bool,
    },
    /// Delete generations.
    Rm {
        /// The generations.
        #[arg(required = true)]
        generations: Vec<u64>,
        /// Skip the garbage collection afterwards.
        #[arg(long)]
        no_gc: bool,
    },
}

/// `laft boot` commands.
#[derive(Debug, Subcommand)]
pub(crate) enum BootCommand {
    /// List the boot menu entries.
    List,
    /// Show the boot record of a generation [default: the booted one].
    Show {
        /// The generation.
        generation: Option<u64>,
    },
    /// Show boot events.
    Log {
        /// Only events about this generation.
        generation: Option<u64>,
    },
    /// Make a generation the boot default without activating it.
    SetDefault {
        /// The generation.
        generation: u64,
    },
    /// Record the booted generation as good.
    MarkGood,
    /// Render the EFI system partition again from the generations.
    Install,
}

/// Options of `laft gc`.
#[derive(Debug, Args)]
pub(crate) struct GcArgs {
    /// Show what would be deleted.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Explain every decision.
    #[arg(long)]
    pub(crate) explain: bool,
    /// Keep at least this many newest generations.
    #[arg(long, value_name = "N")]
    pub(crate) keep_generations: Option<u64>,
    /// Keep every generation younger than this many days.
    #[arg(long, value_name = "D")]
    pub(crate) keep_days: Option<u64>,
}

/// Options of `laft verify`.
#[derive(Debug, Args)]
pub(crate) struct VerifyArgs {
    /// Packages to verify. The packages of the generation by default.
    pub(crate) pkgs: Vec<String>,
    /// Verify every store entry and every generation.
    #[arg(long)]
    pub(crate) all: bool,
    /// Hash every file.
    #[arg(long)]
    pub(crate) full: bool,
    /// Check that generation and view files are the store's files.
    #[arg(long)]
    pub(crate) links: bool,
    /// The generation [default: the active one].
    #[arg(long = "gen", value_name = "N")]
    pub(crate) generation: Option<u64>,
}

/// `laft etc` commands.
#[derive(Debug, Subcommand)]
pub(crate) enum EtcCommand {
    /// List managed paths that differ from the active generation.
    Status,
    /// Show how live files differ from the active generation.
    Diff {
        /// Paths below /etc. Every differing path by default.
        paths: Vec<String>,
    },
    /// Restore managed paths.
    Reset {
        /// Paths below /etc.
        #[arg(required = true)]
        paths: Vec<String>,
    },
}

/// Options of `laft rescue`.
#[derive(Debug, Args)]
pub(crate) struct RescueArgs {
    /// The generation [default: the default one].
    #[arg(long = "gen", value_name = "N")]
    pub(crate) generation: Option<u64>,
    /// The command and its arguments. Root's shell by default.
    #[arg(last = true)]
    pub(crate) command: Vec<String>,
}

/// `laft repo` commands.
#[derive(Debug, Subcommand)]
pub(crate) enum RepoCommand {
    /// Sync indexes now.
    Sync {
        /// Repositories. Every enabled one by default.
        names: Vec<String>,
    },
    /// List the configured repositories.
    List,
    /// Forget the accepted index and keys of a repository.
    Reset {
        /// The repository.
        name: String,
    },
}

/// `laft view` commands.
#[derive(Debug, Subcommand)]
pub(crate) enum ViewCommand {
    /// List the views.
    List,
    /// Show a view.
    Show {
        /// The view.
        name: String,
    },
    /// Rebuild views with the system view pinned.
    Build {
        /// Views. Every view by default.
        names: Vec<String>,
        /// Apply options.
        #[command(flatten)]
        apply: ApplyArgs,
    },
}

/// `laft svc` commands.
#[derive(Debug, Subcommand)]
pub(crate) enum SvcCommand {
    /// Show the state of services.
    Status {
        /// Services. Every enabled one by default.
        names: Vec<String>,
    },
    /// Show a service's spec.
    Show {
        /// The service.
        name: String,
    },
    /// Start services until the next apply or boot.
    Start {
        /// The services.
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// Stop services until the next apply or boot.
    Stop {
        /// The services.
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// Restart services.
    Restart {
        /// The services.
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// Print a service's log.
    Log {
        /// The service.
        name: String,
        /// How many lines to print.
        #[arg(short = 'n', default_value_t = 50)]
        lines: usize,
        /// Keep printing new lines.
        #[arg(short = 'f')]
        follow: bool,
    },
    /// Start a service, from its run script.
    #[command(hide = true)]
    Exec {
        /// The service backend.
        #[command(flatten)]
        backend: Backend,
        /// Start it detached and wait until it is ready.
        #[arg(long)]
        background: bool,
        /// The detached half of a background start.
        #[arg(long, conflicts_with = "background")]
        detached: bool,
        /// The service.
        name: String,
    },
    /// Run a oneshot's up command.
    #[command(hide = true)]
    Up {
        /// The service backend.
        #[command(flatten)]
        backend: Backend,
        /// The service.
        name: String,
    },
    /// Run a oneshot's down command.
    #[command(hide = true)]
    Down {
        /// The service backend.
        #[command(flatten)]
        backend: Backend,
        /// The service.
        name: String,
    },
    /// Handle the end of a service process.
    #[command(hide = true)]
    Finish {
        /// The service backend.
        #[command(flatten)]
        backend: Backend,
        /// The service.
        name: String,
        /// The exit code.
        #[arg(allow_hyphen_values = true)]
        exit_code: i32,
        /// The signal.
        #[arg(allow_hyphen_values = true)]
        signal: i32,
    },
    /// Write a service's log.
    #[command(hide = true)]
    Logger {
        /// The service backend.
        #[command(flatten)]
        backend: Backend,
        /// The service.
        name: String,
    },
    /// Stop the longruns of the early boot graph.
    #[command(hide = true)]
    StopEarly {
        /// The service backend.
        #[command(flatten)]
        backend: Backend,
    },
    /// Watch a service until it is ready.
    #[command(hide = true)]
    Ready {
        /// The service backend.
        #[command(flatten)]
        backend: Backend,
        /// The process that becomes the service.
        #[arg(long)]
        parent: u32,
        /// The service.
        name: String,
    },
}

/// The service backend option of the launcher commands.
#[derive(Debug, Args)]
pub(crate) struct Backend {
    /// The service backend. Only runit exists.
    #[arg(long, value_name = "NAME", default_value = "runit")]
    pub(crate) backend: BackendName,
}

/// A service backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum BackendName {
    /// runit.
    Runit,
}

/// The output formats of `laft eval`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum EvalFormat {
    /// JSON.
    Json,
    /// TOML.
    Toml,
    /// YAML.
    Yaml,
}

/// The shells `laft completions` supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Shell {
    /// bash.
    Bash,
    /// zsh.
    Zsh,
    /// fish.
    Fish,
}

/// Internal commands.
#[derive(Debug, Subcommand)]
pub(crate) enum InternalCommand {
    /// The store worker of a live transaction.
    Worker,
}
