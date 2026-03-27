use clap::Parser;

use super::commands::Commands;

#[derive(Parser, Debug)]
#[command(
    name = "ferret",
    about = "Git repository manager — bookmark, navigate, and inspect repos",
    version = "0.1.0"
)]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Commands>,
}
