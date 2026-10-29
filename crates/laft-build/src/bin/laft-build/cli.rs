//! The command line of `laft-build`.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Builds Laft packages from recipes.
#[derive(Debug, Parser)]
#[command(name = "laft-build", version, about)]
pub(crate) struct Cli {
    /// The build root. Defaults to `$LAFT_BUILD_ROOT`, then to
    /// `$XDG_DATA_HOME/laft-build` or `~/.local/share/laft-build`.
    #[arg(long, value_name = "DIR")]
    pub(crate) root: Option<PathBuf>,
    /// A recipe repository, lowest priority first. Repeat for overlays.
    /// Defaults to the colon separated list in `$LAFT_RECIPES`.
    #[arg(long = "recipes", value_name = "DIR")]
    pub(crate) recipes: Vec<PathBuf>,
    /// Print machine readable JSON on standard output.
    #[arg(long)]
    pub(crate) json: bool,
    /// What to do.
    #[command(subcommand)]
    pub(crate) command: Command,
}

/// The commands.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Print the evaluated recipe of a package, with its rendered phase
    /// scripts and environment.
    Eval {
        /// The package, optionally with `:variant`. Without a variant,
        /// every variant it builds is printed.
        package: String,
    },
    /// Show what would be built, in order, with the trigger codes.
    Plan {
        /// The packages. Without any, every package of the repositories.
        packages: Vec<String>,
        /// Options shared with `build`.
        #[command(flatten)]
        planning: Planning,
    },
    /// Build packages, then everything the changes make stale.
    Build {
        /// The packages, each optionally with `:variant`.
        #[arg(required = true)]
        packages: Vec<String>,
        /// Options shared with `plan`.
        #[command(flatten)]
        planning: Planning,
        /// Options of builds.
        #[command(flatten)]
        options: BuildOptions,
        /// Keep the working tree of every build.
        #[arg(long)]
        keep_work: bool,
        /// Open a shell in the sandbox of a build that fails.
        #[arg(long)]
        shell_on_failure: bool,
    },
    /// Fetch and verify the sources of packages.
    Fetch {
        /// The packages.
        #[arg(required = true)]
        packages: Vec<String>,
        /// Extra source mirrors, tried after the repositories' own.
        #[arg(long = "mirror", value_name = "URL")]
        mirrors: Vec<String>,
    },
    /// Run the phases of a build up to one phase, then open a shell in its
    /// sandbox.
    Shell {
        /// The package, optionally with `:variant`.
        package: String,
        /// The last phase to run, for example `configure`. Without it, the
        /// shell opens before the first phase.
        #[arg(long, value_name = "PHASE")]
        after: Option<String>,
        /// Options of builds.
        #[command(flatten)]
        options: BuildOptions,
    },
    /// Rebuild the latest build of a package with half the jobs elsewhere
    /// and compare the store ids of its outputs.
    CheckRepro {
        /// The package, optionally with `:variant`.
        package: String,
    },
    /// Type check and evaluate recipes and run the recipe checks.
    Lint {
        /// The packages. Without any, every package of the repositories.
        packages: Vec<String>,
    },
    /// Build packages in the given order without sandbox or build-side
    /// store, with the tools of a bootstrap system. The result is not
    /// hermetic and must never be published.
    Bootstrap {
        /// The root of the bootstrap system, whose `usr/bin` provides the
        /// tools. Inside a chroot of that system, this is `/`.
        #[arg(long, value_name = "DIR")]
        root: PathBuf,
        /// Print what is up to date and what would be built, without
        /// building.
        #[arg(long)]
        dry_run: bool,
        /// Options of builds.
        #[command(flatten)]
        options: BuildOptions,
        /// The packages, in build order.
        #[arg(required = true)]
        packages: Vec<String>,
    },
    /// Remove working trees, staging files and old sources.
    Gc {
        /// Also remove cached sources not used for this many days.
        #[arg(long, value_name = "DAYS")]
        sources_older_than: Option<u64>,
    },
}

/// How a run is planned.
#[derive(Debug, Args)]
pub(crate) struct Planning {
    /// Build the named packages even when they are up to date.
    #[arg(long)]
    pub(crate) rebuild: bool,
    /// Let the dependents of this package use its published package instead
    /// of waiting for its build, which breaks a cycle.
    #[arg(long = "use-published", value_name = "PKG")]
    pub(crate) use_published: Vec<String>,
}

/// How builds run.
#[derive(Debug, Args)]
pub(crate) struct BuildOptions {
    /// Skip check phases.
    #[arg(long)]
    pub(crate) no_check: bool,
    /// The number of parallel jobs inside a build. Defaults to the number
    /// of CPUs.
    #[arg(long, value_name = "N")]
    pub(crate) jobs: Option<u32>,
    /// The longest one phase may run, in hours.
    #[arg(long, value_name = "HOURS", default_value_t = 6)]
    pub(crate) timeout_hours: u64,
    /// Extra source mirrors, tried after the repositories' own.
    #[arg(long = "mirror", value_name = "URL")]
    pub(crate) mirrors: Vec<String>,
}
