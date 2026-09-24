use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "lxcup", version, about = "Managed resource update manager")]
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
    entry(std::env::args_os());
}

fn entry(args: impl IntoIterator<Item = std::ffi::OsString>) {
    lxcup_observability::init("lxcup-cli");
    let cli = Cli::parse_from(args);

    if let Some(Command::Version) = cli.command {
        println!("lxcup {}", lxcup_core::VERSION);
    }
}

#[cfg(test)]
mod tests {
    use super::entry;

    #[test]
    fn entry_accepts_version_and_default_invocations() {
        entry(["lxcup-cli", "version"].map(Into::into));
        entry(["lxcup-cli"].map(Into::into));
    }
}
