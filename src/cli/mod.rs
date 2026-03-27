pub mod args;
pub mod commands;

use clap::Parser;

use args::Args;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const YELLOW: &str = "\x1b[33m";
const DIM: &str = "\x1b[2m";

pub fn run() {
    let args = Args::parse();

    match args.command {
        Some(commands::Commands::Add(add)) => {
            if let Err(e) = commands::add::execute(&add) {
                eprintln!("\x1b[31mError:\x1b[0m {}", e);
                std::process::exit(1);
            }
        }
        Some(commands::Commands::Remove(remove)) => {
            if let Err(e) = commands::remove::execute(&remove) {
                eprintln!("\x1b[31mError:\x1b[0m {}", e);
                std::process::exit(1);
            }
        }
        Some(commands::Commands::List(list)) => {
            if let Err(e) = commands::list::execute(&list) {
                eprintln!("\x1b[31mError:\x1b[0m {}", e);
                std::process::exit(1);
            }
        }
        Some(commands::Commands::Repo(repo)) => {
            if let Err(e) = commands::repo::execute(&repo) {
                eprintln!("\x1b[31mError:\x1b[0m {}", e);
                std::process::exit(1);
            }
        }
        Some(commands::Commands::Goto(goto)) => {
            if let Err(e) = commands::goto::execute(&goto) {
                eprintln!("\x1b[31mError:\x1b[0m {}", e);
                std::process::exit(1);
            }
        }
        Some(commands::Commands::Init(init)) => {
            if let Err(e) = commands::init::execute(&init) {
                eprintln!("\x1b[31mError:\x1b[0m {}", e);
                std::process::exit(1);
            }
        }
        Some(commands::Commands::Doctor(doctor)) => {
            if let Err(e) = commands::doctor::execute(&doctor) {
                eprintln!("\x1b[31mError:\x1b[0m {}", e);
                std::process::exit(1);
            }
        }
        Some(commands::Commands::Scan(scan)) => {
            if let Err(e) = commands::scan::execute(&scan) {
                eprintln!("\x1b[31mError:\x1b[0m {}", e);
                std::process::exit(1);
            }
        }
        Some(commands::Commands::Refresh(refresh)) => {
            if let Err(e) = commands::refresh::execute(&refresh) {
                eprintln!("\x1b[31mError:\x1b[0m {}", e);
                std::process::exit(1);
            }
        }
        Some(commands::Commands::Config(config)) => {
            if let Err(e) = commands::config::execute(&config) {
                eprintln!("\x1b[31mError:\x1b[0m {}", e);
                std::process::exit(1);
            }
        }
        None => {
            println!(
                "  {}{}Ferret{} {}v0.1.0{} — Git Repository Manager",
                BOLD, CYAN, RESET, DIM, RESET
            );
            println!();
            println!("  {}Commands:{}", YELLOW, RESET);
            println!("    {}add{}       Register a repository", BOLD, RESET);
            println!("    {}remove{}    Remove a repository", BOLD, RESET);
            println!(
                "    {}list{}      List registered repositories",
                BOLD, RESET
            );
            println!("    {}repo{}      Show repository info", BOLD, RESET);
            println!("    {}goto{}      Navigate to a repository", BOLD, RESET);
            println!("    {}init{}      Generate shell integration", BOLD, RESET);
            println!("    {}doctor{}    Check Ferret health", BOLD, RESET);
            println!(
                "    {}scan{}      Scan directories for git repositories",
                BOLD, RESET
            );
            println!(
                "    {}refresh{}   Refresh branch and tracker info for repositories",
                BOLD, RESET
            );
            println!(
                "    {}config{}    View and modify configuration",
                BOLD, RESET
            );
            println!();
            println!(
                "  {}{}Use 'ferret --help' for full documentation.{}",
                DIM, YELLOW, RESET
            );
        }
    }
}
