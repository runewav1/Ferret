pub mod add;
pub mod config;
pub mod doctor;
pub mod goto;
pub mod init;
pub mod list;
pub mod pull;
pub mod push;
pub mod refresh;
pub mod remove;
pub mod repo;
pub mod scan;

use clap::Subcommand;

/// Available Ferret commands for managing your repository registry.
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Add a repository to the registry (local path, remote URL, or link)
    Add(add::AddArgs),
    /// Remove a repository from the registry (or unlink its remote)
    Remove(remove::RemoveArgs),
    /// List registered repositories with filtering and sorting options
    List(list::ListArgs),
    /// Show detailed info about a specific repository (status, commits, diffs)
    Repo(repo::RepoArgs),
    /// Navigate to a repository (editor, explorer, shell, or browser)
    Goto(goto::GotoArgs),
    /// Generate shell integration for `fg` function (enables actual directory changes)
    Init(init::InitArgs),
    /// Diagnose Ferret's configuration, registry, and environment
    Doctor(doctor::DoctorArgs),
    /// Scan directories for git repositories and optionally add them to the registry
    Scan(scan::ScanArgs),
    /// Refresh branch and tracker info for one or all registered repositories
    Refresh(refresh::RefreshArgs),
    /// View and modify Ferret configuration
    Config(config::ConfigArgs),
    /// Push local commits to remote for registered repositories
    Push(push::PushArgs),
    /// Pull remote changes for registered repositories
    Pull(pull::PullArgs),
}
