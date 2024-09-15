mod command;

use command::insert::Insert;
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(author, version, about)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
    #[clap(long, short, global = true)]
    verbose: bool,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    Insert(Insert),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    use Command::*;
    match cli.command {
        Insert(m) => m.run(),
    }
}