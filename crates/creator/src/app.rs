use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crossterm::event::{KeyCode, KeyEvent};
use nimbus_vault::{
    config::{OriginConfig, VaultConfig},
    crypt::{CryptKey, write_key_file},
    error::VaultError,
};
use ratatui::{Terminal, backend::Backend};

use crate::{
    builder::{FieldSpec, OriginKind, absolute_answer},
    event::{AppEvent, Event, EventHandler},
};

/// The wizard's current step. Progress is otherwise linear — `Esc` cancels the whole wizard
/// rather than stepping back — with two exceptions: a refused or failed save returns to
/// `SavePath`, and a key path that is taken returns to `KeyPath`, so a bad path can be
/// corrected without losing everything already typed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    Name,
    RootId,
    SelectOrigin,
    Field(usize),
    /// Whether to encrypt the vault at rest: `y` or `n`.
    Encrypt,
    /// Where to write the new key file (encrypted vaults only).
    KeyPath,
    SavePath,
    Confirm,
    /// The new key, generated in memory on `Confirm`, shown once. Nothing is on disk yet: the
    /// key file and then the config are written only after the user types [`BACKUP_PHRASE`].
    KeyBackup,
}

/// What the user types to confirm they have kept a copy of a new vault's key.
pub const BACKUP_PHRASE: &str = "I have a backup";

/// What a finished wizard leaves on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The vault config written, or `None` if the wizard stopped without saving one.
    pub config: Option<PathBuf>,
    /// A key file the wizard wrote for a config it then didn't save (the save failed and the
    /// user gave up). Kept, never deleted: it is the only copy of a key the user confirmed
    /// backing up. The caller should tell the user, with [`kept_key_notice`].
    pub unsaved_key_file: Option<PathBuf>,
}

/// What to tell the user about a key file the wizard wrote and kept without saving a config.
pub fn kept_key_notice(path: &Path) -> String {
    format!(
        "a key file was written and kept at {0}; the vault config wasn't saved, so write a config \
         for it by hand (key_file = {0}) or delete the key",
        path.display()
    )
}

/// The interactive vault-config builder. Construct with [`App::new`], then drive it either via
/// [`App::run`] (owns a blocking terminal event loop) or by feeding [`App::handle_key_event`]
/// directly (e.g. from tests, or a caller with its own event loop). Deliberately not `Debug`:
/// it can hold a vault's key.
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
    /// Whether the vault is to be encrypted at rest.
    pub(crate) encrypt: bool,
    /// Where its key file goes, as typed.
    pub(crate) key_path: String,
    /// The new vault's key, held in memory from `Confirm` until the config is saved or the
    /// wizard is cancelled (it wipes itself when dropped). Kept across a key-path collision,
    /// so the key the user backed up is the one that gets written.
    pub(crate) key: Option<CryptKey>,
    /// The key file this wizard wrote. Never removed: once written, it is the only copy of a
    /// key the user has confirmed backing up. A retried config save doesn't write it again.
    pub(crate) key_file_written: Option<PathBuf>,
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
            encrypt: false,
            key_path: String::new(),
            key: None,
            key_file_written: None,
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

    /// Whether the wizard is still awaiting input. `false` once it has completed or been
    /// cancelled — lets a caller drive it via [`Self::handle_key_event`] directly (feeding it
    /// key events from their own event loop) instead of via [`Self::run`], and know when to
    /// stop and collect the outcome with [`Self::into_outcome`].
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Consumes the wizard, returning the path a config was written to (`Some`), or `None` if
    /// it was cancelled. Only meaningful once [`Self::is_running`] is `false`; see
    /// [`Self::into_result`] for the key file a cancelled wizard may have kept.
    pub fn into_outcome(self) -> Option<PathBuf> {
        self.outcome
    }

    /// The key file this wizard wrote and kept without saving a config: `Some` only once it
    /// has stopped, with no config saved, after writing a key (a failed save, then `Esc`).
    pub fn unsaved_key_file(&self) -> Option<&Path> {
        match (self.running, &self.outcome) {
            (false, None) => self.key_file_written.as_deref(),
            _ => None,
        }
    }

    /// Consumes the wizard, returning everything it left on disk. Only meaningful once
    /// [`Self::is_running`] is `false`.
    pub fn into_result(self) -> Outcome {
        Outcome {
            unsaved_key_file: self.unsaved_key_file().map(Path::to_path_buf),
            config: self.outcome,
        }
    }

    /// Runs the wizard's blocking event loop against `terminal`, returning the path a config
    /// was written to (`Some`), or `None` if the user cancelled. The caller owns terminal
    /// init/restore. [`Self::run_for_outcome`] also reports a key file kept without a config.
    pub fn run<B: Backend>(self, terminal: &mut Terminal<B>) -> color_eyre::Result<Option<PathBuf>>
    where
        B::Error: std::error::Error + Send + Sync + 'static,
    {
        Ok(self.run_for_outcome(terminal)?.config)
    }

    /// Runs the wizard's blocking event loop against `terminal`, returning its [`Outcome`].
    /// The caller owns terminal init/restore, and should show [`kept_key_notice`] for an
    /// `unsaved_key_file` once the terminal is restored.
    pub fn run_for_outcome<B: Backend>(
        mut self,
        terminal: &mut Terminal<B>,
    ) -> color_eyre::Result<Outcome>
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
        Ok(self.into_result())
    }

    /// Applies a single key press to the wizard's state. Public so callers/tests can drive the
    /// wizard without a live terminal.
    pub fn handle_key_event(&mut self, key: KeyEvent) -> color_eyre::Result<()> {
        self.error = None;
        match self.step {
            Step::SelectOrigin => self.handle_select_origin(key),
            Step::Encrypt => self.handle_encrypt(key),
            Step::Confirm => self.handle_confirm(key)?,
            _ => self.handle_text_input(key),
        }
        Ok(())
    }

    /// Stops the wizard without saving a config. Before the backup phrase nothing is on disk;
    /// after it, a key file already written is left alone, never deleted.
    fn cancel(&mut self) {
        self.key = None;
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
                self.input.clear();
                self.step = if self.fields.is_empty() {
                    Step::Encrypt
                } else {
                    Step::Field(0)
                };
            }
            _ => {}
        }
    }

    /// `y` encrypts the vault (next: where its key goes), `n` doesn't (next: where the config
    /// goes). Nothing else answers it, so the choice is always deliberate.
    fn handle_encrypt(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.cancel(),
            KeyCode::Char('y' | 'Y') => {
                self.encrypt = true;
                self.key_path = default_key_path(&self.name);
                self.input = self.key_path.clone();
                self.step = Step::KeyPath;
            }
            KeyCode::Char('n' | 'N') => {
                self.encrypt = false;
                self.go_to_save_path();
            }
            _ => self.error = Some("press y to encrypt this vault, n not to".into()),
        }
    }

    /// Moves on to the save path, offering the default the first time and the earlier answer
    /// when coming back (after a key-path collision, say).
    fn go_to_save_path(&mut self) {
        if self.save_path.is_empty() {
            self.save_path = default_save_path(&self.name);
        }
        self.input = self.save_path.clone();
        self.step = Step::SavePath;
    }

    /// The config the answers describe: the chosen origin, wrapped in a `crypt` origin with
    /// the key file when the vault is encrypted.
    fn build_config(&self) -> Option<VaultConfig> {
        let kind = self.origin_kind?;
        let mut origin_config = kind.build(&self.values);
        if self.encrypt {
            // The file actually written, once there is one, so the config provably names it.
            let key_file = self
                .key_file_written
                .clone()
                .unwrap_or_else(|| self.key_file());
            origin_config = OriginConfig::Crypt {
                key_file: Some(key_file),
                key_env: None,
                inner: Box::new(origin_config),
            };
        }
        let root_id = if self.root_id.is_empty() {
            Default::default()
        } else {
            self.root_id.as_str().into()
        };
        Some(VaultConfig::new(self.name.clone(), root_id, origin_config))
    }

    /// The key file's path as it will be written and recorded: made absolute the way the
    /// wizard's other path answers are.
    pub(crate) fn key_file(&self) -> PathBuf {
        absolute_answer(self.key_path.clone())
    }

    fn handle_confirm(&mut self, key: KeyEvent) -> color_eyre::Result<()> {
        match key.code {
            KeyCode::Esc => self.cancel(),
            KeyCode::Enter => {
                let path = PathBuf::from(&self.save_path);

                // `save` writes through, so an existing file here would be destroyed — and since
                // the default path is derived from the vault name, reusing a name is exactly how
                // you'd land on one. Back to the save-path step so the path can be edited rather
                // than losing everything typed so far.
                if path.exists() {
                    self.error = Some(format!(
                        "{} already exists — choose another path (or another vault name)",
                        path.display()
                    ));
                    self.input = self.save_path.clone();
                    self.step = Step::SavePath;
                    return Ok(());
                }

                match (self.encrypt, &self.key_file_written) {
                    (false, _) | (true, Some(_)) => self.save(),
                    (true, None) => self.show_key(),
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Generates the vault's key in memory, unless one is already held, and shows it once
    /// ([`Step::KeyBackup`]). Writes nothing: the key file waits for the backup phrase.
    fn show_key(&mut self) {
        if self.key.is_none() {
            match CryptKey::generate() {
                Ok(key) => self.key = Some(key),
                Err(e) => {
                    self.error = Some(format!("failed to generate a key: {e}"));
                    return;
                }
            }
        }
        self.input.clear();
        self.step = Step::KeyBackup;
    }

    /// The backup phrase was typed: writes the key file (unless an earlier attempt already
    /// did), then the config. A key path that is taken goes back to [`Step::KeyPath`] with the
    /// same key held: an existing key file is never overwritten.
    fn write_key_then_save(&mut self) {
        if self.key_file_written.is_none() {
            let Some(key) = &self.key else {
                self.error = Some("no key to write".into());
                return;
            };
            let key_file = self.key_file();
            if let Some(dir) = key_file.parent()
                && let Err(e) = create_private_dirs(dir)
            {
                self.error = Some(format!(
                    "failed to create the key directory {}: {e}",
                    dir.display()
                ));
                return;
            }
            match write_key_file(&key_file, key) {
                Ok(()) => self.key_file_written = Some(key_file),
                Err(VaultError::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    self.error = Some(format!(
                        "already exists, choose another path (the wizard never replaces a key \
                         file): {}",
                        key_file.display()
                    ));
                    self.input = self.key_path.clone();
                    self.step = Step::KeyPath;
                    return;
                }
                Err(e) => {
                    self.error = Some(format!("failed to write the key: {e}"));
                    return;
                }
            }
        }
        self.save();
    }

    /// Saves the config and finishes the wizard. A failure goes back to the save path; for an
    /// encrypted vault whose key file is already written, the error says where the key is, and
    /// a retry saves without writing the key again.
    fn save(&mut self) {
        let Some(config) = self.build_config() else {
            self.error = Some("no origin type selected".into());
            return;
        };
        let path = PathBuf::from(&self.save_path);
        match config.save(&path) {
            Ok(()) => {
                self.key = None;
                self.outcome = Some(path);
                self.events.send(AppEvent::Quit);
                self.running = false;
            }
            Err(e) => {
                // The essential part first: a narrow terminal may still cut the end.
                self.error = Some(match &self.key_file_written {
                    Some(key_file) => format!(
                        "key kept at {}: failed to save the config: {e}",
                        key_file.display()
                    ),
                    None => format!("failed to save the config: {e}"),
                });
                self.input = self.save_path.clone();
                self.step = Step::SavePath;
            }
        }
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
            Step::SavePath | Step::KeyPath => self.input.trim().is_empty(),
            Step::SelectOrigin | Step::Encrypt | Step::Confirm | Step::KeyBackup => false,
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
                self.input.clear();
                self.step = if i + 1 < self.fields.len() {
                    Step::Field(i + 1)
                } else {
                    Step::Encrypt
                };
            }
            Step::KeyPath => {
                self.key_path = self.input.trim().to_string();
                self.go_to_save_path();
            }
            Step::KeyBackup => {
                if self.input.trim().eq_ignore_ascii_case(BACKUP_PHRASE) {
                    self.input.clear();
                    self.write_key_then_save();
                } else {
                    self.error = Some(format!(
                        "type \"{BACKUP_PHRASE}\" once the key is saved somewhere safe"
                    ));
                }
            }
            Step::SavePath => {
                self.save_path = self.input.trim().to_string();
                self.input.clear();
                self.step = Step::Confirm;
            }
            Step::SelectOrigin | Step::Encrypt | Step::Confirm => {}
        }
    }
}

/// Creates `dir` and any missing parents. On Unix each directory this call creates is `0700`,
/// so a new keys directory is private to its owner; an existing directory's mode is never
/// changed. Each level is created on its own, so one that appears meanwhile counts as existing.
fn create_private_dirs(dir: &Path) -> std::io::Result<()> {
    let mut missing: Vec<&Path> = dir
        .ancestors()
        .take_while(|d| !d.as_os_str().is_empty() && !d.exists())
        .collect();
    missing.reverse();
    for level in missing {
        match create_private_dir(level) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            result => result?,
        }
    }
    Ok(())
}

/// Creates the single directory `dir`: on Unix created `0700` by the `mkdir` itself, which the
/// umask can only narrow. No follow-up `chmod`: it would go by path, and a directory swapped
/// for a symlink in between would have the symlink's target changed instead.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(dir)
    }
    #[cfg(not(unix))]
    std::fs::create_dir(dir)
}

/// Where a new vault's key file is offered to go: `keys/<name>.key` under the config home,
/// next to the `vaults/` directory but apart from it, so copying the vault configs around
/// doesn't copy the keys. Still editable on the key-path step.
fn default_key_path(name: &str) -> String {
    nimbus_vault::config_home()
        .join("keys")
        .join(format!("{name}.key"))
        .display()
        .to_string()
}

/// Where a newly built vault config is offered to be saved: the conventional vaults directory,
/// named after the vault.
///
/// A full path rather than a bare `<name>.toml`, so the wizard doesn't quietly drop configs into
/// whatever directory the program happened to be launched from — and so a vault created from the
/// TUI ends up somewhere it can be found again. Still editable on the save-path step.
fn default_save_path(name: &str) -> String {
    VaultConfig::default_path(name).display().to_string()
}

/// Directory entries under `input`'s parent whose name starts with `input`'s last component,
/// each formatted back into a path string (directories get a trailing `/`). Understands a
/// leading `~` the way a shell would.
fn path_suggestions(input: &str) -> Vec<String> {
    let input = crate::builder::expand_tilde(input);
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
