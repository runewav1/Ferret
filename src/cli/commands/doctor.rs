use crate::pathutil;
use crate::registry::RegistryManager;
use clap::Args;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const DIM: &str = "\x1b[2m";

/// Diagnose Ferret's configuration, registry, and environment health.
#[derive(Args, Debug, Clone)]
#[command(
    about = "Check Ferret health and show storage paths",
    long_about = "Diagnose Ferret's configuration, registry, and environment.\n\n\
        Without flags, runs a health check on config and registry.\n\
        Use --paths to show resolved file paths.\n\n\
        Examples:\n  \
        ferret doctor              # Health check\n  \
        ferret doctor --paths      # Show storage paths"
)]
pub struct DoctorArgs {
    /// Show the resolved paths for config, data, and cache files
    #[arg(long)]
    pub paths: bool,
}

pub fn execute(args: &DoctorArgs) -> crate::error::Result<()> {
    if args.paths {
        show_paths()?;
    } else {
        health_check()?;
    }
    Ok(())
}

fn show_paths() -> crate::error::Result<()> {
    println!("  {}{}Ferret paths{}", BOLD, YELLOW, RESET);
    println!();

    // Config dir
    if let Some(config_dir) = pathutil::ferret_config_dir() {
        println!(
            "    {}Config dir:{} {}",
            YELLOW,
            RESET,
            config_dir.display()
        );
        println!(
            "    {}Config file:{} {}",
            YELLOW,
            RESET,
            config_dir.join("config.toml").display()
        );
    } else {
        println!("    {}Config dir:{} {}unknown{}", YELLOW, RESET, RED, RESET);
    }

    // Data dir
    if let Some(data_dir) = pathutil::ferret_data_dir() {
        println!("    {}Data dir:{}   {}", YELLOW, RESET, data_dir.display());
        println!(
            "    {}Registry:{}   {}",
            YELLOW,
            RESET,
            data_dir.join("registry.json").display()
        );
    } else {
        println!("    {}Data dir:{}   {}unknown{}", YELLOW, RESET, RED, RESET);
    }

    println!();
    Ok(())
}

fn health_check() -> crate::error::Result<()> {
    println!("  {}{}Ferret doctor{}", BOLD, YELLOW, RESET);
    println!();

    let mut issues = 0;

    // --- Config check ---
    print!("    {}Config file...{}", YELLOW, RESET);
    match crate::config::FerretConfig::config_file_path() {
        Ok(config_path) => {
            if config_path.exists() {
                match crate::config::FerretConfig::load() {
                    Ok(_) => println!(" {}{}ok{}", GREEN, BOLD, RESET),
                    Err(e) => {
                        println!(" {}{}error{}: {}", RED, BOLD, RESET, e);
                        println!("      {}File:{} {}", DIM, RESET, config_path.display());
                        issues += 1;
                    }
                }
            } else {
                println!(" {}{}not found{} (using defaults)", DIM, BOLD, RESET);
            }
        }
        Err(e) => {
            println!(" {}{}error{}: {}", RED, BOLD, RESET, e);
            issues += 1;
        }
    }

    // --- Registry check ---
    print!("    {}Registry...{}", YELLOW, RESET);
    match crate::registry::storage::RegistryStorage::new() {
        Ok(storage) => {
            let reg_path = storage.path();
            if reg_path.exists() {
                match storage.load() {
                    Ok(store) => {
                        // Validate each entry
                        let mut bad_entries = 0;
                        for entry in &store.entries {
                            if entry.id.is_empty() || entry.name.is_empty() {
                                bad_entries += 1;
                            }
                            // Check if local paths still exist
                            if let Some(path) = &entry.local_path {
                                if !path.exists() {
                                    bad_entries += 1;
                                }
                            }
                        }
                        if bad_entries > 0 {
                            println!(
                                " {}{}warn{}: {} entries have issues ({} total)",
                                YELLOW,
                                BOLD,
                                RESET,
                                bad_entries,
                                store.entries.len()
                            );
                            issues += 1;
                        } else {
                            println!(
                                " {}{}ok{} ({} entries)",
                                GREEN,
                                BOLD,
                                RESET,
                                store.entries.len()
                            );
                        }
                    }
                    Err(e) => {
                        println!(" {}{}corrupt{}: {}", RED, BOLD, RESET, e);
                        println!("      {}File:{} {}", DIM, RESET, reg_path.display());
                        issues += 1;
                    }
                }
            } else {
                println!(" {}{}empty{} (no entries yet)", DIM, BOLD, RESET);
            }
        }
        Err(e) => {
            println!(" {}{}error{}: {}", RED, BOLD, RESET, e);
            issues += 1;
        }
    }

    // --- git-tracker path validation ---
    print!("    {}Repo paths...{}", YELLOW, RESET);
    match crate::registry::storage::RegistryStorage::new() {
        Ok(storage) => {
            match storage.load() {
                Ok(store) => {
                    let local_entries: Vec<_> = store
                        .entries
                        .iter()
                        .filter(|e| e.local_path.is_some())
                        .collect();

                    if local_entries.is_empty() {
                        println!(" {}{}n/a{} (no local entries)", DIM, BOLD, RESET);
                    } else {
                        let mut missing = Vec::new();
                        let mut moved = Vec::new();
                        let mut stale_branch = Vec::new();

                        for entry in &local_entries {
                            let path = entry.local_path.as_ref().unwrap();

                            if !path.exists() {
                                // Path is gone — try to locate via fingerprint
                                if let Some(fp_hash) = &entry.fingerprint_hash {
                                    // Build a quick relocator snapshot to search the parent dir
                                    use git_tracker::identity::FingerprintKind;
                                    use git_tracker::relocator::{Relocator, RelocatorConfig};
                                    use git_tracker::snapshot::RepoSnapshot;

                                    let snap = RepoSnapshot {
                                        fingerprint: git_tracker::RepoFingerprint::from_raw(
                                            fp_hash.clone(),
                                            FingerprintKind::HeadCommit,
                                            None,
                                        ),
                                        workdir: path.clone(),
                                        git_dir: path.join(".git"),
                                        name: entry.name.clone(),
                                        is_bare: false,
                                        is_linked_worktree: false,
                                        snapshotted_at: 0,
                                        scan_root: path.parent().unwrap_or(path).to_path_buf(),
                                        current_branch: entry.current_branch.clone(),
                                    };

                                    let reloc_config = RelocatorConfig::builder()
                                        .max_depth(5)
                                        .min_score(40)
                                        .build();

                                    match Relocator::new(reloc_config).locate(&snap) {
                                        Ok(candidate) => {
                                            moved.push((
                                                entry.name.clone(),
                                                path.clone(),
                                                candidate.new_path.clone(),
                                            ));
                                        }
                                        Err(_) => {
                                            missing.push((entry.name.clone(), path.clone()));
                                        }
                                    }
                                } else {
                                    missing.push((entry.name.clone(), path.clone()));
                                }
                            } else {
                                // Path exists — check whether the branch info is stale
                                // (i.e. the registry branch differs from what git reports now)
                                if let Ok(live_branch) = git_tracker::get_branch_info(path) {
                                    let live_name = if live_branch.is_detached {
                                        None
                                    } else {
                                        Some(live_branch.name.clone())
                                    };
                                    if live_name != entry.current_branch {
                                        stale_branch.push((
                                            entry.name.clone(),
                                            entry.current_branch.clone(),
                                            live_name,
                                        ));
                                    }
                                }
                            }
                        }

                        let total_issues = missing.len() + moved.len() + stale_branch.len();
                        if total_issues == 0 {
                            println!(
                                " {}{}ok{} ({} local paths verified)",
                                GREEN,
                                BOLD,
                                RESET,
                                local_entries.len(),
                            );
                        } else {
                            println!(
                                " {}{}warn{}: {} issue(s) detected",
                                YELLOW, BOLD, RESET, total_issues,
                            );
                            issues += 1;

                            for (name, path) in &missing {
                                println!(
                                    "      {}missing:{} '{}' — path no longer exists",
                                    RED, RESET, name,
                                );
                                println!("        {}Was: {}{}", DIM, path.display(), RESET,);
                            }

                            for (name, old_path, new_path) in &moved {
                                println!(
                                    "      {}→ moved:{} '{}' — found at new location",
                                    YELLOW, RESET, name,
                                );
                                println!("        {}From: {}{}", DIM, old_path.display(), RESET,);
                                println!("        {}  To: {}{}", DIM, new_path.display(), RESET,);
                                println!(
                                    "        {}Hint: run 'ferret remove {}' then 'ferret add --path {}' to re-register{}",
                                    DIM, name, new_path.display(), RESET,
                                );
                            }

                            for (name, cached, live) in &stale_branch {
                                let cached_str = cached.as_deref().unwrap_or("(detached)");
                                let live_str = live.as_deref().unwrap_or("(detached)");
                                println!(
                                    "      {}stale branch:{} '{}' — registry says '{}', git reports '{}'",
                                    YELLOW, RESET, name, cached_str, live_str,
                                );
                                println!(
                                    "        {}Hint: run 'ferret refresh {}' to update{}",
                                    DIM, name, RESET,
                                );
                            }
                        }
                    }
                }
                Err(_) => {
                    println!(" {}{}skip{} (registry unreadable)", DIM, BOLD, RESET);
                }
            }
        }
        Err(_) => {
            println!(" {}{}skip{} (storage unavailable)", DIM, BOLD, RESET);
        }
    }

    // --- Branch freshness summary ---
    print!("    {}Branch data...{}", YELLOW, RESET);
    match RegistryManager::new() {
        Ok(manager) => match manager.get_all() {
            Ok(entries) => {
                let local_with_branch: Vec<_> = entries
                    .iter()
                    .filter(|e| e.local_path.is_some() && e.current_branch.is_some())
                    .collect();
                let no_branch_info: Vec<_> = entries
                    .iter()
                    .filter(|e| {
                        e.local_path.is_some() && e.current_branch.is_none() && !e.head_detached
                    })
                    .collect();

                if entries.iter().all(|e| e.local_path.is_none()) {
                    println!(" {}{}n/a{} (no local entries)", DIM, BOLD, RESET);
                } else if no_branch_info.is_empty() {
                    println!(
                        " {}{}ok{} ({} with branch info)",
                        GREEN,
                        BOLD,
                        RESET,
                        local_with_branch.len(),
                    );
                } else {
                    println!(
                        " {}{}warn{}: {} local entries missing branch info",
                        YELLOW,
                        BOLD,
                        RESET,
                        no_branch_info.len(),
                    );
                    for entry in &no_branch_info {
                        println!(
                            "      {}• '{}'{} — no branch data (run 'ferret refresh {}')",
                            DIM, entry.name, RESET, entry.name,
                        );
                    }
                    issues += 1;
                }
            }
            Err(_) => {
                println!(" {}{}skip{} (registry unreadable)", DIM, BOLD, RESET);
            }
        },
        Err(_) => {
            println!(" {}{}skip{} (manager unavailable)", DIM, BOLD, RESET);
        }
    }

    // --- Data dir check ---
    print!("    {}Data dir...{}", YELLOW, RESET);
    match pathutil::ferret_data_dir() {
        Some(data_dir) => {
            if data_dir.exists() {
                println!(" {}{}ok{}", GREEN, BOLD, RESET);
            } else {
                println!(" {}{}not created yet{}", DIM, BOLD, RESET);
            }
        }
        None => {
            println!(" {}{}cannot determine{}", RED, BOLD, RESET);
            issues += 1;
        }
    }

    println!();
    if issues == 0 {
        println!("    {}{}All checks passed{}", GREEN, BOLD, RESET);
    } else {
        println!("    {}{}{} issue(s) found{}", RED, BOLD, issues, RESET);
    }
    println!();

    Ok(())
}
