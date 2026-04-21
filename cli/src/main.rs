use clap::{Parser, Subcommand};

mod cmd;

#[derive(Parser)]
#[command(name = "stui", version, about = "STUI plugin author CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(subcommand)]
    Plugin(cmd::plugin::PluginCmd),
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    match cli.command {
        Command::Plugin(plugin_cmd) => cmd::plugin::run(plugin_cmd),
    }
}
