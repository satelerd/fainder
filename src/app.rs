use anyhow::Result;

use crate::config::Config;
use crate::model::SearchResult;
use crate::providers;

pub fn print_results(results: &[SearchResult]) {
    for result in results {
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
            "[{}] {}  {}  {}\n  {}\n  {}\n",
            result.provider, updated, result.title, cwd, result.matched_in, result.resume_command
        );
    }
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
