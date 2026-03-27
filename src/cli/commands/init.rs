use clap::Args;
use std::path::PathBuf;

const RESET: &str = "\x1b[0m";
const YELLOW: &str = "\x1b[33m";

/// Generate shell integration for ferret goto.
///
/// Outputs a shell function that wraps `ferret goto` so the directory
/// actually changes in the current terminal. Add the output to your
/// shell profile (e.g., $PROFILE for PowerShell, .bashrc for Bash).
#[derive(Args, Debug, Clone)]
#[command(
    about = "Generate shell integration for ferret goto",
    long_about = "Generate shell integration so `ferret goto` changes the current directory.\n\n\
        The default action prints the snippet to stdout for you to copy into your profile.\n\
        Use --file to write it to a file instead.\n\n\
        Supported shells: pwsh, powershell, bash, zsh, fish, cmd, nushell\n\n\
        Examples:\n  \
        ferret init pwsh              # Print PowerShell snippet\n  \
        ferret init bash --file       # Write Bash snippet to file"
)]
pub struct InitArgs {
    /// Target shell type: pwsh, powershell, bash, zsh, fish, cmd, nushell
    pub shell: String,

    /// Write the snippet to a file instead of stdout
    #[arg(long)]
    pub file: Option<String>,
}

pub fn execute(args: &InitArgs) -> crate::error::Result<()> {
    let shell = args.shell.to_lowercase();
    let snippet = get_snippet(&shell)?;

    if let Some(file_path) = &args.file {
        let path = PathBuf::from(file_path);
        std::fs::write(&path, &snippet)?;
        eprintln!("{}Written to:{} {}", YELLOW, RESET, path.display());
    } else {
        println!("{}", snippet);
    }

    Ok(())
}

fn get_snippet(shell: &str) -> crate::error::Result<String> {
    match shell {
        "pwsh" | "powershell" => Ok(POWERSHELL.to_string()),
        "bash" => Ok(BASH.to_string()),
        "zsh" => Ok(ZSH.to_string()),
        "fish" => Ok(FISH.to_string()),
        "cmd" => Ok(CMD.to_string()),
        "nushell" | "nu" => Ok(NUSHELL.to_string()),
        _ => Err(crate::error::FerretError::ConfigError(format!(
            "Unknown shell '{}'. Supported: pwsh, bash, zsh, fish, cmd, nushell",
            shell
        ))),
    }
}

const POWERSHELL: &str = r#"# Ferret shell integration — add to $PROFILE
function fg {
    $path = ferret goto @args
    if ($path) { Invoke-Expression $path }
}
"#;

const BASH: &str = r#"# Ferret shell integration — add to ~/.bashrc
fg() {
    local path
    path=$(ferret goto "$@")
    if [ -n "$path" ]; then
        eval "$path"
    fi
}
"#;

const ZSH: &str = r#"# Ferret shell integration — add to ~/.zshrc
fg() {
    local path
    path=$(ferret goto "$@")
    if [ -n "$path" ]; then
        eval "$path"
    fi
}
"#;

const FISH: &str = r#"# Ferret shell integration — add to ~/.config/fish/config.fish
function fg
    set -l path (ferret goto $argv)
    if test -n "$path"
        eval $path
    end
end
"#;

const CMD: &str = r#"@echo off
REM Ferret shell integration — save as fg.bat somewhere on PATH
for /f "delims=" %%i in ('ferret goto %*') do set _fg_path=%%i
if defined _fg_path (
    cd /d "%_fg_path%"
    set _fg_path=
)
"#;

const NUSHELL: &str = r#"# Ferret shell integration — add to config.nu
def fg [...args] {
    let path = (ferret goto ...$args | str trim)
    if ($path | is-not-empty) {
        cd $path
    }
}
"#;
