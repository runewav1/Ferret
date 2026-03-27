use clap::{Args, Subcommand};

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const YELLOW: &str = "\x1b[33m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";

/// View and modify Ferret configuration.
///
/// Without subcommands, displays all current configuration values.
/// Use `get` and `set` to read or write individual keys.
#[derive(Args, Debug, Clone)]
#[command(
    about = "View and modify Ferret configuration",
    long_about = "View and modify Ferret's configuration.\n\n\
        Without subcommands, shows all current configuration values.\n\
        Use `ferret config get <key>` to read a value.\n\
        Use `ferret config set <key> <value>` to write a value.\n\n\
        Examples:\n  \
        ferret config                         # Show all values\n  \
        ferret config get refresh_interval    # Show refresh config\n  \
        ferret config set default_editor code # Set editor to code\n  \
        ferret config set refresh_interval.enabled true\n  \
        ferret config set refresh_interval.interval 10m\n  \
        ferret config set refresh_interval.fields branch,remote\n  \
        ferret config set refresh_interval.excluded +Lime  # Add exclusion\n  \
        ferret config set refresh_interval.excluded -Lime  # Remove exclusion"
)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: Option<ConfigCommand>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum ConfigCommand {
    /// Get a configuration value
    Get {
        /// The config key to read (e.g., "default_editor", "refresh_interval", "refresh_interval.enabled")
        key: String,
    },
    /// Set a configuration value
    Set {
        /// The config key to write
        key: String,
        /// The value to set
        value: String,
    },
}

pub fn execute(args: &ConfigArgs) -> crate::error::Result<()> {
    let config = crate::config::FerretConfig::load()?;

    match &args.command {
        None => {
            show_all(&config);
            Ok(())
        }
        Some(ConfigCommand::Get { key }) => {
            get_key(&config, key);
            Ok(())
        }
        Some(ConfigCommand::Set { key, value }) => set_key(config, key, value),
    }
}

fn show_all(config: &crate::config::FerretConfig) {
    println!("  {}{}Ferret configuration{}", BOLD, YELLOW, RESET);
    println!();

    let config_path = crate::config::FerretConfig::config_file_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "(unknown)".to_string());
    println!("    {}File:{}     {}", YELLOW, RESET, config_path);
    println!();

    println!(
        "    {}default_editor:{}      {}",
        YELLOW,
        RESET,
        config
            .default_editor
            .as_deref()
            .unwrap_or("(not set, using: code)")
    );
    println!(
        "    {}default_explorer:{}    {}",
        YELLOW,
        RESET,
        config
            .default_explorer
            .as_deref()
            .unwrap_or("(not set, using: system default)")
    );
    println!(
        "    {}default_shell:{}       {}",
        YELLOW,
        RESET,
        config
            .default_shell
            .as_deref()
            .unwrap_or("(not set, using: bash)")
    );
    println!(
        "    {}always_rescan_langs:{} {}",
        YELLOW,
        RESET,
        config
            .always_rescan_languages
            .map(|v| v.to_string())
            .unwrap_or_else(|| "(not set)".to_string())
    );

    println!();
    println!("    {}{}refresh_interval:{}", BOLD, CYAN, RESET);
    println!(
        "      {}enabled:{}   {}",
        YELLOW, RESET, config.refresh_interval.enabled
    );
    println!(
        "      {}interval:{}  {}",
        YELLOW, RESET, config.refresh_interval.interval
    );
    if config.refresh_interval.fields.is_empty() {
        println!("      {}fields:{}    (all)", YELLOW, RESET);
    } else {
        println!(
            "      {}fields:{}    {}",
            YELLOW,
            RESET,
            config.refresh_interval.fields.join(", ")
        );
    }
    if config.refresh_interval.excluded.is_empty() {
        println!("      {}excluded:{}  (none)", YELLOW, RESET);
    } else {
        println!(
            "      {}excluded:{}  {}",
            YELLOW,
            RESET,
            config.refresh_interval.excluded.join(", ")
        );
    }
    println!();
}

fn get_key(config: &crate::config::FerretConfig, key: &str) {
    match key {
        "default_editor" => println!(
            "{}",
            config.default_editor.as_deref().unwrap_or("(not set)")
        ),
        "default_explorer" => println!(
            "{}",
            config.default_explorer.as_deref().unwrap_or("(not set)")
        ),
        "default_shell" => println!("{}", config.default_shell.as_deref().unwrap_or("(not set)")),
        "always_rescan_languages" | "always_rescan_langs" => {
            println!(
                "{}",
                config
                    .always_rescan_languages
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "(not set)".to_string())
            );
        }
        "refresh_interval" => {
            println!("enabled:  {}", config.refresh_interval.enabled);
            println!("interval: {}", config.refresh_interval.interval);
            println!(
                "fields:   {}",
                if config.refresh_interval.fields.is_empty() {
                    "(all)".to_string()
                } else {
                    config.refresh_interval.fields.join(", ")
                }
            );
            println!(
                "excluded: {}",
                if config.refresh_interval.excluded.is_empty() {
                    "(none)".to_string()
                } else {
                    config.refresh_interval.excluded.join(", ")
                }
            );
        }
        "refresh_interval.enabled" => println!("{}", config.refresh_interval.enabled),
        "refresh_interval.interval" => println!("{}", config.refresh_interval.interval),
        "refresh_interval.fields" => println!(
            "{}",
            if config.refresh_interval.fields.is_empty() {
                "(all)".to_string()
            } else {
                config.refresh_interval.fields.join(", ")
            }
        ),
        "refresh_interval.excluded" => println!(
            "{}",
            if config.refresh_interval.excluded.is_empty() {
                "(none)".to_string()
            } else {
                config.refresh_interval.excluded.join(", ")
            }
        ),
        _ => eprintln!("{}Unknown config key: '{}'{}", RED, key, RESET),
    }
}

fn set_key(
    mut config: crate::config::FerretConfig,
    key: &str,
    value: &str,
) -> crate::error::Result<()> {
    match key {
        "default_editor" => config.default_editor = Some(value.to_string()),
        "default_explorer" => config.default_explorer = Some(value.to_string()),
        "default_shell" => config.default_shell = Some(value.to_string()),
        "always_rescan_languages" | "always_rescan_langs" => {
            config.always_rescan_languages = Some(parse_bool(value)?);
        }
        "refresh_interval.enabled" => {
            config.refresh_interval.enabled = parse_bool(value)?;
        }
        "refresh_interval.interval" => {
            // Validate the interval before saving
            crate::config::file::parse_duration(value)?;
            config.refresh_interval.interval = value.to_string();
        }
        "refresh_interval.fields" => {
            if value.eq_ignore_ascii_case("all") || value.is_empty() {
                config.refresh_interval.fields = Vec::new();
            } else {
                config.refresh_interval.fields =
                    value.split(',').map(|s| s.trim().to_string()).collect();
            }
        }
        "refresh_interval.excluded" => {
            // Support +name (add) and -name (remove) syntax
            if let Some(name) = value.strip_prefix('+') {
                if !config
                    .refresh_interval
                    .excluded
                    .iter()
                    .any(|e| e.eq_ignore_ascii_case(name))
                {
                    config.refresh_interval.excluded.push(name.to_string());
                }
            } else if let Some(name) = value.strip_prefix('-') {
                config
                    .refresh_interval
                    .excluded
                    .retain(|e| !e.eq_ignore_ascii_case(name));
            } else {
                // Replace entire list
                config.refresh_interval.excluded =
                    value.split(',').map(|s| s.trim().to_string()).collect();
            }
        }
        _ => {
            eprintln!("{}Unknown config key: '{}'{}", RED, key, RESET);
            return Ok(());
        }
    }

    config.save()?;
    println!("  {}{}Set {} = {}{}", GREEN, BOLD, key, value, RESET);
    Ok(())
}

fn parse_bool(value: &str) -> crate::error::Result<bool> {
    match value.to_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(crate::error::FerretError::ConfigError(format!(
            "invalid boolean: '{}' (use true/false)",
            value
        ))),
    }
}
