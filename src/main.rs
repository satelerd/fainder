mod app;
mod clipboard;
mod config;
mod model;
mod providers;
mod search;
mod tui;

use anyhow::Result;
use clap::{Parser, Subcommand};
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
        #[arg(long, value_delimiter = ',')]
        provider: Vec<ProviderKind>,
        #[arg(long)]
        regex: bool,
        #[arg(long)]
        json: bool,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show discovered providers, paths, and local session counts.
    Doctor,
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
            limit,
        }) => {
            let options = SearchOptions {
                query,
                providers: provider,
                regex,
                limit,
                full_text: true,
            };
            let results = search::search(&config, &options)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                app::print_results(&results);
            }
        }
        Some(Commands::Doctor) => app::doctor(&config)?,
        None => tui::run(config)?,
    }

    Ok(())
}
