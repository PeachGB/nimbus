use clap::{CommandFactory, Parser};

use crate::event::AppEvent;

/// Parses a `:`-command-line's typed text (already split on whitespace, no quoting support —
/// same limitation as `nimbus-cli`'s REPL) into the [`AppEvent`] it names.
#[derive(Parser)]
#[command(name = "nimbus", no_binary_name = true)]
struct Cli {
    #[command(subcommand)]
    command: AppEvent,
}

pub fn parse(line: &str) -> Result<AppEvent, String> {
    Cli::try_parse_from(line.split_whitespace())
        .map(|cli| cli.command)
        .map_err(|e| e.to_string())
}

/// One `usage — description` pair per `:` command, taken from the same clap definitions `parse`
/// uses, so the help overlay can't drift out of sync with what's actually accepted.
pub fn command_summaries() -> Vec<(String, String)> {
    Cli::command()
        .get_subcommands()
        .map(|sub| {
            // `render_usage` yields e.g. "Usage: cp <PATH> <DESTINATION> [VAULT]"; strip the
            // label (and the binary name, if this clap version includes it) to leave the
            // argument shape, which is the part worth showing next to the description.
            let rendered = sub.clone().render_usage().to_string();
            let usage = rendered
                .trim()
                .strip_prefix("Usage:")
                .map(str::trim)
                .and_then(|rest| rest.strip_prefix("nimbus ").or(Some(rest)))
                .filter(|usage| !usage.is_empty())
                .unwrap_or(sub.get_name())
                .to_string();
            let about = sub
                .get_about()
                .map(|about| about.to_string())
                .unwrap_or_default();
            (usage, about)
        })
        .collect()
}
