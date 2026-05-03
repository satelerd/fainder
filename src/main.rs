mod app;
mod clipboard;
mod config;
mod model;
mod providers;
mod search;
mod tui;

use std::process::Command;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use model::{ProviderKind, SearchOptions};

#[derive(Parser)]
#[command(name = "fainder")]
#[command(about = "Live universal finder for local AI agent conversations")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Search conversations without opening the TUI.
    Search {
        query: String,
        /// Providers to search. Accepts comma-separated values.
        #[arg(long, value_delimiter = ',')]
        provider: Vec<ProviderKind>,
        /// Treat the query as a case-insensitive regular expression.
        #[arg(long)]
        regex: bool,
        /// Print machine-readable JSON.
        #[arg(long, conflicts_with_all = ["command_only", "copy", "open"])]
        json: bool,
        /// Print only the selected result's resume command.
        #[arg(long, conflicts_with_all = ["json", "copy", "open"])]
        command_only: bool,
        /// Copy the selected result's resume command to the clipboard.
        #[arg(long)]
        copy: bool,
        /// Execute the selected result's resume command.
        #[arg(long)]
        open: bool,
        /// One-based result index used by --command-only, --copy, and --open.
        #[arg(long, default_value_t = 1)]
        select: usize,
        /// Include snippets and latest messages in human-readable output.
        #[arg(long)]
        preview: bool,
        /// Search scope.
        #[arg(long, value_enum, default_value_t = SearchScope::All)]
        scope: SearchScope,
        /// Maximum number of results.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show discovered providers, paths, and local session counts.
    Doctor,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SearchScope {
    /// Search metadata and full transcript content.
    All,
    /// Search title, path, and recent messages only.
    Metadata,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = config::Config::load()?;

    match cli.command {
        Some(Commands::Search {
            query,
            provider,
            regex,
            json,
            command_only,
            copy,
            open,
            select,
            preview,
            scope,
            limit,
        }) => {
            let options = SearchOptions {
                query,
                providers: provider,
                regex,
                limit,
                full_text: matches!(scope, SearchScope::All),
            };
            let results = search::search(&config, &options)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&results)?);
                return Ok(());
            }

            if command_only {
                if let Some(result) = app::selected_result(&results, select)? {
                    println!("{}", result.resume_command);
                }
                return Ok(());
            }

            if copy {
                if let Some(result) = app::selected_result(&results, select)? {
                    clipboard::copy(&result.resume_command)?;
                    eprintln!("Copied: {}", result.resume_command);
                }
            }

            if open {
                if let Some(result) = app::selected_result(&results, select)? {
                    Command::new("sh")
                        .arg("-lc")
                        .arg(&result.resume_command)
                        .status()?;
                }
                return Ok(());
            }

            app::print_results(&results, preview);
        }
        Some(Commands::Doctor) => app::doctor(&config)?,
        None => tui::run(config)?,
    }

    Ok(())
}
