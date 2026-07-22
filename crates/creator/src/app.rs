use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crossterm::event::{KeyCode, KeyEvent};
use nimbus_vault::config::VaultConfig;
use ratatui::{Terminal, backend::Backend};

use crate::{
    builder::{FieldSpec, OriginKind},
    event::{AppEvent, Event, EventHandler},
};

/// The wizard's current step. There is no backward navigation: `Esc` always cancels the whole
/// wizard rather than stepping back, keeping the state machine linear.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Name,
    RootId,
    SelectOrigin,
    Field(usize),
    SavePath,
    Confirm,
}

/// The interactive vault-config builder. Construct with [`App::new`], then drive it either via
/// [`App::run`] (owns a blocking terminal event loop) or by feeding [`App::handle_key_event`]
/// directly (e.g. from tests, or a caller with its own event loop).
pub struct App {
    pub(crate) step: Step,
    pub(crate) input: String,
    pub(crate) name: String,
    pub(crate) root_id: String,
    pub(crate) origin_idx: usize,
    pub(crate) origin_kind: Option<OriginKind>,
    pub(crate) fields: Vec<FieldSpec>,
    pub(crate) values: HashMap<String, String>,
    pub(crate) save_path: String,
    pub(crate) error: Option<String>,
    pub(crate) suggestions: Vec<String>,
    running: bool,
    outcome: Option<PathBuf>,
    events: EventHandler,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        App {
            step: Step::Name,
            input: String::new(),
            name: String::new(),
            root_id: String::new(),
            origin_idx: 0,
            origin_kind: None,
            fields: Vec::new(),
            values: HashMap::new(),
            save_path: String::new(),
            error: None,
            suggestions: Vec::new(),
            running: true,
            outcome: None,
            events: EventHandler::new(),
        }
    }

    pub fn current_field(&self) -> Option<&FieldSpec> {
        match self.step {
            Step::Field(i) => self.fields.get(i),
            _ => None,
        }
    }

    /// Runs the wizard's blocking event loop against `terminal`, returning the path a config
    /// was written to (`Some`), or `None` if the user cancelled. The caller owns terminal
    /// init/restore.
    pub fn run<B: Backend>(
        mut self,
        terminal: &mut Terminal<B>,
    ) -> color_eyre::Result<Option<PathBuf>>
    where
        B::Error: std::error::Error + Send + Sync + 'static,
    {
        while self.running {
            terminal.draw(|frame| crate::ui::render(&self, frame))?;
            match self.events.next()? {
                Event::Tick => {}
                Event::Crossterm(event) => {
                    if let crossterm::event::Event::Key(key_event) = event
                        && key_event.kind == crossterm::event::KeyEventKind::Press
                    {
                        self.handle_key_event(key_event)?;
                    }
                }
                Event::App(AppEvent::Quit) => self.running = false,
            }
        }
        Ok(self.outcome)
    }

    /// Applies a single key press to the wizard's state. Public so callers/tests can drive the
    /// wizard without a live terminal.
    pub fn handle_key_event(&mut self, key: KeyEvent) -> color_eyre::Result<()> {
        self.error = None;
        match self.step {
            Step::SelectOrigin => self.handle_select_origin(key),
            Step::Confirm => self.handle_confirm(key)?,
            _ => self.handle_text_input(key),
        }
        Ok(())
    }

    fn cancel(&mut self) {
        self.outcome = None;
        self.events.send(AppEvent::Quit);
        self.running = false;
    }

    fn handle_select_origin(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.cancel(),
            KeyCode::Up => {
                self.origin_idx = self.origin_idx.saturating_sub(1);
            }
            KeyCode::Down => {
                self.origin_idx = (self.origin_idx + 1).min(OriginKind::ALL.len() - 1);
            }
            KeyCode::Enter => {
                let kind = OriginKind::ALL[self.origin_idx];
                self.origin_kind = Some(kind);
                self.fields = kind.fields();
                self.step = if self.fields.is_empty() {
                    self.save_path = format!("{}.toml", self.name);
                    self.input = self.save_path.clone();
                    Step::SavePath
                } else {
                    self.input.clear();
                    Step::Field(0)
                };
            }
            _ => {}
        }
    }

    fn handle_confirm(&mut self, key: KeyEvent) -> color_eyre::Result<()> {
        match key.code {
            KeyCode::Esc => self.cancel(),
            KeyCode::Enter => {
                let Some(kind) = self.origin_kind else {
                    self.error = Some("no origin type selected".into());
                    return Ok(());
                };
                let origin_config = kind.build(&self.values);
                let root_id = if self.root_id.is_empty() {
                    Default::default()
                } else {
                    self.root_id.as_str().into()
                };
                let config = VaultConfig::new(self.name.clone(), root_id, origin_config);
                let path = PathBuf::from(&self.save_path);
                match config.save(&path) {
                    Ok(()) => {
                        self.outcome = Some(path);
                        self.events.send(AppEvent::Quit);
                        self.running = false;
                    }
                    Err(e) => self.error = Some(format!("failed to save: {e}")),
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_text_input(&mut self, key: KeyEvent) {
        if !matches!(key.code, KeyCode::Tab) {
            self.suggestions.clear();
        }
        match key.code {
            KeyCode::Esc => self.cancel(),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(c) => self.input.push(c),
            KeyCode::Enter => self.commit_text_input(),
            KeyCode::Tab => self.try_complete_path(),
            _ => {}
        }
    }

    fn try_complete_path(&mut self) {
        let completable = self
            .current_field()
            .map(|f| f.path_completable)
            .unwrap_or(false);
        if !completable {
            return;
        }
        let matches = path_suggestions(&self.input);
        match matches.len() {
            0 => self.error = Some(format!("no matches for '{}'", self.input)),
            1 => self.input = matches[0].clone(),
            _ => {
                let common = longest_common_prefix(&matches);
                if common.len() > self.input.len() {
                    self.input = common;
                }
                self.suggestions = matches;
            }
        }
    }

    fn commit_text_input(&mut self) {
        let required_and_empty = match self.step {
            Step::Name => self.input.trim().is_empty(),
            Step::RootId => false,
            Step::Field(i) => self
                .fields
                .get(i)
                .map(|f| !f.optional && self.input.trim().is_empty())
                .unwrap_or(false),
            Step::SavePath => self.input.trim().is_empty(),
            Step::SelectOrigin | Step::Confirm => false,
        };
        if required_and_empty {
            self.error = Some("this field is required".into());
            return;
        }

        match self.step {
            Step::Name => {
                self.name = self.input.trim().to_string();
                self.input.clear();
                self.step = Step::RootId;
            }
            Step::RootId => {
                self.root_id = self.input.trim().to_string();
                self.input.clear();
                self.step = Step::SelectOrigin;
            }
            Step::Field(i) => {
                if let Some(field) = self.fields.get(i) {
                    self.values
                        .insert(field.key.to_string(), self.input.clone());
                }
                if i + 1 < self.fields.len() {
                    self.input.clear();
                    self.step = Step::Field(i + 1);
                } else {
                    self.save_path = format!("{}.toml", self.name);
                    self.input = self.save_path.clone();
                    self.step = Step::SavePath;
                }
            }
            Step::SavePath => {
                self.save_path = self.input.trim().to_string();
                self.input.clear();
                self.step = Step::Confirm;
            }
            Step::SelectOrigin | Step::Confirm => {}
        }
    }
}

/// Expands a leading `~` or `~/...` to the user's home directory, so typed paths behave like
/// they would in a shell. Left untouched if there's no leading `~` or the home dir is unknown.
fn expand_tilde(input: &str) -> String {
    let Some(rest) = input.strip_prefix('~') else {
        return input.to_string();
    };
    let Some(home) = dirs::home_dir() else {
        return input.to_string();
    };
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    if rest.is_empty() {
        home.to_string_lossy().into_owned()
    } else {
        home.join(rest).to_string_lossy().into_owned()
    }
}

/// Directory entries under `input`'s parent whose name starts with `input`'s last component,
/// each formatted back into a path string (directories get a trailing `/`). Understands a
/// leading `~` the way a shell would.
fn path_suggestions(input: &str) -> Vec<String> {
    let input = expand_tilde(input);
    let path = Path::new(&input);
    let (dir, dir_is_dot, prefix) = if input.is_empty() || input.ends_with('/') {
        (
            PathBuf::from(if input.is_empty() { "." } else { &input }),
            input.is_empty(),
            String::new(),
        )
    } else {
        let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
        let prefix = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        match dir {
            Some(dir) => (dir.to_path_buf(), false, prefix),
            None => (PathBuf::from("."), true, prefix),
        }
    };

    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut matches: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(&prefix) {
                return None;
            }
            let mut full = if dir_is_dot {
                name
            } else {
                dir.join(&name).to_string_lossy().into_owned()
            };
            if entry.path().is_dir() {
                full.push('/');
            }
            Some(full)
        })
        .collect();
    matches.sort();
    matches
}

fn longest_common_prefix(items: &[String]) -> String {
    let Some(first) = items.first() else {
        return String::new();
    };
    let mut prefix = first.clone();
    for item in &items[1..] {
        while !item.starts_with(prefix.as_str()) {
            prefix.pop();
            if prefix.is_empty() {
                return prefix;
            }
        }
    }
    prefix
}
