use std::cell::RefCell;
use std::rc::Rc;

use nimbus_core::app::App;
use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper, Result};

const SUBCOMMANDS: &[&str] = &[
    "ls", "vaults", "select", "new", "cd", "put", "get", "delete", "cp", "mv", "push", "pull",
    "exit",
];

/// Completes subcommand names as the first word of the line, and vault/directory names as
/// the first argument of `cd`. Path completion for other commands is not implemented yet.
pub struct NimbusHelper {
    app: Rc<RefCell<App>>,
}

impl NimbusHelper {
    pub fn new(app: Rc<RefCell<App>>) -> Self {
        NimbusHelper { app }
    }

    fn complete_cd(&self, word: &str) -> Vec<Pair> {
        let app = self.app.clone();
        let word = word.to_string();
        // Completion only ever runs synchronously inside `readline()`, never concurrently
        // with the `dispatch` borrow below, so holding this borrow across the await is safe.
        #[allow(clippy::await_holding_refcell_ref)]
        let names = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { app.borrow().cd_completions(&word).await })
        });
        names
            .into_iter()
            .map(|name| Pair {
                display: name.clone(),
                replacement: name,
            })
            .collect()
    }
}

impl Completer for NimbusHelper {
    type Candidate = Pair;

    fn complete(&self, line: &str, pos: usize, _ctx: &Context<'_>) -> Result<(usize, Vec<Pair>)> {
        let start = line[..pos]
            .rfind(char::is_whitespace)
            .map(|i| i + 1)
            .unwrap_or(0);

        let before = &line[..start];
        if before.trim().is_empty() {
            let word = &line[start..pos];
            let candidates = SUBCOMMANDS
                .iter()
                .filter(|cmd| cmd.starts_with(word))
                .map(|cmd| Pair {
                    display: cmd.to_string(),
                    replacement: cmd.to_string(),
                })
                .collect();
            return Ok((start, candidates));
        }

        let mut words = before.split_whitespace();
        let is_cd_first_argument = words.next() == Some("cd") && words.next().is_none();
        if is_cd_first_argument {
            let word = &line[start..pos];
            return Ok((start, self.complete_cd(word)));
        }

        Ok((start, Vec::new()))
    }
}

impl Hinter for NimbusHelper {
    type Hint = String;
}

impl Highlighter for NimbusHelper {}
impl Validator for NimbusHelper {}
impl Helper for NimbusHelper {}

#[cfg(test)]
#[path = "tests/completion.rs"]
mod tests;
