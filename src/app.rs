use anyhow::{Result, bail};

use crate::config::Config;
use crate::model::SearchResult;
use crate::providers;

pub fn print_results(results: &[SearchResult], preview: bool) {
    for (index, result) in results.iter().enumerate() {
        let cwd = result
            .cwd
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "-".to_string());
        let updated = result
            .updated_at
            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{}. [{}] {}  {}  {}\n  matched: {}  score: {}\n  command: {}\n",
            index + 1,
            result.provider,
            updated,
            result.title,
            cwd,
            result.matched_in,
            result.score,
            result.resume_command
        );
        if preview {
            if !result.snippets.is_empty() {
                println!("  snippets:");
                for snippet in &result.snippets {
                    println!("    - {}", one_line(snippet));
                }
            }
            if !result.latest_messages.is_empty() {
                println!("  latest:");
                for message in &result.latest_messages {
                    println!("    - {}", one_line(message));
                }
            }
            println!();
        }
    }
}

pub fn selected_result(results: &[SearchResult], select: usize) -> Result<Option<&SearchResult>> {
    if results.is_empty() {
        return Ok(None);
    }
    if select == 0 || select > results.len() {
        bail!(
            "--select must be between 1 and {} for the current result set",
            results.len()
        );
    }
    Ok(results.get(select - 1))
}

pub fn doctor(config: &Config) -> Result<()> {
    let reports = providers::doctor(config)?;
    println!("Fainder doctor\n");
    for report in reports {
        let status = if report.exists { "ok" } else { "missing" };
        println!(
            "{:<9} {:<8} {:>5} sessions  {}",
            report.provider,
            status,
            report.sessions,
            report.path.display()
        );
        for warning in report.warnings {
            println!("  warning: {warning}");
        }
    }
    Ok(())
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
