//! `teams-cli` — headless Teams client used for protocol exploration (Phase 2),
//! integration tests, and debugging without the GUI.
//!
//! Planned commands (see `TODO.md`):
//! - `teams-cli auth login | logout | status | whoami`   (Phase 3)
//! - `teams-cli conversations | history <conv>`          (Phase 4)
//! - `teams-cli watch | send <conv> <text>`              (Phase 5)
//! - `teams-cli call <user> | answer | hangup`           (Phase 8)
//!
//! The command surface grows phase by phase; right now it only proves the workspace links.

use clap::Parser;

/// Unofficial Microsoft Teams client for Linux (headless).
#[derive(Parser, Debug)]
#[command(name = "teams-cli", version, about, long_about = None)]
struct Cli {
    /// Increase log verbosity (-v = debug, -vv = trace).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,
}

fn main() {
    let cli = Cli::parse();

    let level = match cli.verbose {
        0 => "warn",
        1 => "debug",
        _ => "trace",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level)),
        )
        .with_target(false)
        .init();

    println!("teams-cli {} (workspace link check OK)", env!("CARGO_PKG_VERSION"));
    println!("No commands implemented yet — see TODO.md Phase 3+.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
