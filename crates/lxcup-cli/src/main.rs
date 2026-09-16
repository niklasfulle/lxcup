use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "lxcup", version, about = "Proxmox LXC update manager")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Zeigt die installierte lxcup-Version an.
    Version,
}

fn main() {
    lxcup_observability::init("lxcup-cli");
    let cli = Cli::parse();

    if let Some(Command::Version) = cli.command {
        println!("lxcup {}", lxcup_core::VERSION);
    }
}
