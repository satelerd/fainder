mod app;
mod clipboard;
mod config;
mod model;
mod providers;
mod search;
mod transcript;
mod tui;

use std::process::Command;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use model::{ProviderKind, SearchMode, SearchOptions};
use transcript::{ContextOptions, InspectOptions, RoleFilter};

#[derive(Parser)]
#[command(name = "fainder")]
#[command(about = "Live universal finder for local AI agent conversations")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Show the most recently used conversations.
    Recent {
        /// Providers to list. Accepts comma-separated values.
        #[arg(long, value_delimiter = ',')]
        provider: Vec<ProviderKind>,
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
        /// Include latest messages in human-readable output.
        #[arg(long)]
        preview: bool,
        /// Maximum number of results.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Search conversations without opening the TUI.
    Search {
        query: String,
        /// Providers to search. Accepts comma-separated values.
        #[arg(long, value_delimiter = ',')]
        provider: Vec<ProviderKind>,
        /// Treat the query as a case-insensitive regular expression.
        #[arg(long, conflicts_with = "words")]
        regex: bool,
        /// Match each word independently (AND) instead of the exact phrase.
        #[arg(long, conflicts_with = "regex")]
        words: bool,
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
    /// Inspect one conversation by role, search matches, or turn windows.
    Inspect {
        /// Session reference formatted as provider:id, for example claude:abc123.
        session: String,
        /// Filter by role.
        #[arg(long, default_value_t = RoleFilter::All)]
        role: RoleFilter,
        /// Search inside the selected conversation.
        #[arg(long)]
        find: Option<String>,
        /// Treat --find as a case-insensitive regular expression.
        #[arg(long)]
        regex: bool,
        /// Show all turns compactly.
        #[arg(long)]
        timeline: bool,
        /// Show a single turn and optional context around it.
        #[arg(long)]
        turn: Option<usize>,
        /// Show a window around a turn.
        #[arg(long)]
        around: Option<usize>,
        /// Number of turns before/after --turn or --around.
        #[arg(long, default_value_t = 5)]
        context: usize,
        /// Show the last N matching turns.
        #[arg(long)]
        tail: Option<usize>,
        /// Maximum number of matching turns to show.
        #[arg(long, default_value_t = 40)]
        limit: usize,
        /// Print machine-readable JSON.
        #[arg(long)]
        json: bool,
        /// Print expanded message bodies instead of one-line previews.
        #[arg(long)]
        expand: bool,
    },
    /// Print a chronological transcript window for a selected conversation.
    Context {
        /// Session reference formatted as provider:id, for example codex:019dec17.
        session: String,
        /// First turn to include.
        #[arg(long)]
        from_turn: Option<usize>,
        /// Last turn to include.
        #[arg(long)]
        to_turn: Option<usize>,
        /// Print a window around a turn.
        #[arg(long)]
        around: Option<usize>,
        /// Number of turns before/after --around.
        #[arg(long, default_value_t = 10)]
        context: usize,
        /// Print the last N turns.
        #[arg(long)]
        tail: Option<usize>,
        /// Filter by role. Default prints chronological all roles.
        #[arg(long, default_value_t = RoleFilter::All)]
        role: RoleFilter,
        /// Print large outputs after the token estimate warning.
        #[arg(long)]
        confirm: bool,
        /// Token budget before --confirm is required.
        #[arg(long, default_value_t = 10_000)]
        max_tokens: usize,
        /// Truncate output to this many characters.
        #[arg(long)]
        max_chars: Option<usize>,
        /// Omit tool calls/results.
        #[arg(long)]
        no_tools: bool,
        /// Truncate long tool calls/results.
        #[arg(long)]
        truncate_tools: bool,
        /// Output format.
        #[arg(long, value_enum, default_value_t = ContextFormat::Markdown)]
        format: ContextFormat,
        /// Alias for --format json.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SearchScope {
    /// Search metadata and full transcript content.
    All,
    /// Search title, path, and recent messages only.
    Metadata,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ContextFormat {
    Markdown,
    Json,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = config::Config::load()?;

    match cli.command {
        Some(Commands::Recent {
            provider,
            json,
            command_only,
            copy,
            open,
            select,
            preview,
            limit,
        }) => {
            let results = search::recent(&config, &provider, limit)?;
            print_or_act_on_results(&results, json, command_only, copy, open, select, preview)?;
        }
        Some(Commands::Search {
            query,
            provider,
            regex,
            words,
            json,
            command_only,
            copy,
            open,
            select,
            preview,
            scope,
            limit,
        }) => {
            let mode = if regex {
                SearchMode::Regex
            } else if words {
                SearchMode::Words
            } else {
                SearchMode::Phrase
            };
            let options = SearchOptions {
                query,
                providers: provider,
                mode,
                limit,
                full_text: matches!(scope, SearchScope::All),
            };
            let results = search::search(&config, &options)?;
            print_or_act_on_results(&results, json, command_only, copy, open, select, preview)?;
        }
        Some(Commands::Doctor) => app::doctor(&config)?,
        Some(Commands::Inspect {
            session,
            role,
            find,
            regex,
            timeline,
            turn,
            around,
            context,
            tail,
            limit,
            json,
            expand,
        }) => transcript::inspect(
            &config,
            InspectOptions {
                session_ref: session,
                role,
                find,
                regex,
                timeline,
                turn,
                around,
                context,
                tail,
                limit,
                json,
                expand,
            },
        )?,
        Some(Commands::Context {
            session,
            from_turn,
            to_turn,
            around,
            context,
            tail,
            role,
            confirm,
            max_tokens,
            max_chars,
            no_tools,
            truncate_tools,
            format,
            json,
        }) => transcript::context(
            &config,
            ContextOptions {
                session_ref: session,
                from_turn,
                to_turn,
                around,
                context,
                tail,
                role,
                confirm,
                max_tokens,
                max_chars,
                no_tools,
                truncate_tools,
                json: json || matches!(format, ContextFormat::Json),
            },
        )?,
        None => tui::run(config)?,
    }

    Ok(())
}

fn print_or_act_on_results(
    results: &[model::SearchResult],
    json: bool,
    command_only: bool,
    copy: bool,
    open: bool,
    select: usize,
    preview: bool,
) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(results)?);
        return Ok(());
    }

    if command_only {
        if let Some(result) = app::selected_result(results, select)? {
            println!("{}", result.resume_command);
        }
        return Ok(());
    }

    if copy {
        if let Some(result) = app::selected_result(results, select)? {
            clipboard::copy(&result.resume_command)?;
            eprintln!("Copied: {}", result.resume_command);
        }
    }

    if open {
        if let Some(result) = app::selected_result(results, select)? {
            Command::new("sh")
                .arg("-lc")
                .arg(&result.resume_command)
                .status()?;
        }
        return Ok(());
    }

    app::print_results(results, preview);
    Ok(())
}
