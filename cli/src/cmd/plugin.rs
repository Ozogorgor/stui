use clap::Subcommand;

#[derive(Subcommand)]
pub enum PluginCmd {
    /// Scaffold a new plugin skeleton.
    Init {
        /// Plugin name (short form; will be manifest.plugin.name).
        name: String,
        /// Target directory (defaults to ./<name>-provider).
        #[arg(short, long)]
        dir: Option<std::path::PathBuf>,
    },
    /// Build the plugin to wasm32-wasip1.
    Build {
        #[arg(long)]
        release: bool,
    },
    /// Run plugin tests with the mocked host harness.
    Test,
    /// Lint the plugin manifest + impl surface.
    Lint,
    /// Install the built plugin to ~/.stui/plugins/<name>/ (dev-mode: symlink).
    Install {
        #[arg(long)]
        dev: bool,
    },
}

pub fn run(cmd: PluginCmd) -> anyhow::Result<()> {
    match cmd {
        PluginCmd::Init { name, dir } => crate::cmd::init::run(name, dir),
        PluginCmd::Build { release } => crate::cmd::build::run(release),
        PluginCmd::Test => crate::cmd::test::run(),
        PluginCmd::Lint => crate::cmd::lint::run(),
        PluginCmd::Install { dev } => crate::cmd::install::run(dev),
    }
}
