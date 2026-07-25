use crate::command;
use crate::event::{AppEvent, Event, EventHandler};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nimbus_core::app::App as NimbusApp;
use nimbus_creator::App as CreatorApp;
use nimbus_vault::object::{Object, ObjectId};
use ratatui::{DefaultTerminal, widgets::ListState};
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Default, Debug, PartialEq, Eq)]
pub enum AppMode {
    #[default]
    Root,
    Vault(String),
    New,
    Quit,
}

/// Application.
pub struct App {
    /// Current screen.
    pub mode: AppMode,
    /// Event handler.
    pub events: EventHandler,
    pub nimbus: NimbusApp,
    /// Vault names shown on the root screen.
    pub vaults: Vec<String>,
    pub vault_state: ListState,
    /// Every object in the current working directory, as the vault reported them.
    pub objects: Vec<Object>,
    /// Indices into [`Self::objects`], in display order — what the list actually shows once the
    /// filter, hidden-file setting and sort have been applied. `object_state` indexes *this*,
    /// not `objects`, so a filtered list still selects the right thing.
    pub visible: Vec<usize>,
    pub object_state: ListState,
    /// Last error/info message, shown in the status bar.
    pub status: Option<String>,
    /// `Some(buffer)` while the `:`-command line is open and being typed into.
    pub command: Option<String>,
    /// `Some(wizard)` while the vault creator wizard is up, taking over rendering and key
    /// input. Driven from this app's own event loop (not `nimbus_creator::run`'s blocking,
    /// self-threaded entry point) so the two don't compete for terminal input.
    pub creator: Option<CreatorApp>,
    /// The object waiting to be pasted, yanked with `y` (copy) or `d` (cut).
    pub clipboard: Option<Clipboard>,
    /// `Some(scroll_offset)` while the help overlay is up.
    pub help: Option<u16>,
    /// `Some(pending)` while a destructive action waits on a y/n answer in the status bar.
    pub confirm: Option<Confirm>,
    /// Names marked with space, which bulk operations act on instead of the cursor.
    ///
    /// Names, not indices: a refresh renumbers the list but doesn't rename its contents, so
    /// indices would silently come to mean different objects after any mutation.
    pub marked: HashSet<String>,
    /// `Some(text)` while a `/` filter is narrowing the listing.
    pub filter: Option<String>,
    /// Whether keystrokes are currently going into [`Self::filter`] rather than acting as
    /// bindings; the filter itself stays applied after the user stops typing.
    pub filtering: bool,
    /// `Some(prompt)` while `r` is asking for a new name for the selected object.
    pub rename: Option<RenamePrompt>,
    pub sort: SortKey,
    pub sort_reverse: bool,
    /// Whether dot-prefixed names are listed.
    pub show_hidden: bool,
}

/// An in-progress rename: the object being renamed, and the name being typed for it.
pub struct RenamePrompt {
    /// The object's current name, needed to address it when the rename is submitted — the
    /// cursor may not still be on it by then, and `input` has already been edited away from it.
    pub original: String,
    /// The new name, pre-filled with `original` so a small edit (an extension, a suffix) doesn't
    /// mean retyping the whole thing.
    pub input: String,
}

/// An action held back until the user confirms it, so a single keypress can't destroy anything.
pub struct Confirm {
    /// The question to put in the status bar.
    pub prompt: String,
    /// What to run, in order, on `y`; dropped on anything else. A list so one confirmation can
    /// cover a whole marked selection.
    pub events: Vec<AppEvent>,
}

/// What the object list is ordered by. Directories always come first regardless — a file
/// manager that interleaves them is much harder to scan.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SortKey {
    #[default]
    Name,
    Size,
    Modified,
}

impl SortKey {
    /// Cycles to the next key, for the `s` binding.
    pub fn next(self) -> Self {
        match self {
            SortKey::Name => SortKey::Size,
            SortKey::Size => SortKey::Modified,
            SortKey::Modified => SortKey::Name,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Name => "name",
            SortKey::Size => "size",
            SortKey::Modified => "modified",
        }
    }
}

/// What `y`/`d` picked up, to be copied or moved on the next `p`.
pub struct Clipboard {
    pub entries: Vec<ClipboardEntry>,
    /// Whether to `mv` (cut) rather than `cp` (copy) on paste.
    pub cut: bool,
}

impl Clipboard {
    /// How the clipboard reads in the status bar.
    pub fn label(&self) -> String {
        match self.entries.as_slice() {
            [one] => one.name.clone(),
            many => format!("{} objects", many.len()),
        }
    }
}

pub struct ClipboardEntry {
    /// The fully-qualified `vault:/path` of the yanked object, so a paste still resolves it
    /// after navigating away — including into a different vault.
    pub spec: String,
    /// The object's own name, for display in the status bar.
    pub name: String,
}

impl App {
    pub fn running(&self) -> bool {
        self.mode != AppMode::Quit
    }
}

impl Default for App {
    fn default() -> Self {
        let nimbus = match NimbusApp::init() {
            Ok(a) => a,
            Err(e) => {
                eprintln!("ERROR: {}", e);
                NimbusApp::default()
            }
        };
        let vaults = nimbus.vault_names();
        let mut vault_state = ListState::default();
        if !vaults.is_empty() {
            vault_state.select(Some(0));
        }
        Self {
            mode: AppMode::default(),
            events: EventHandler::new(),
            nimbus,
            vaults,
            vault_state,
            objects: Vec::new(),
            visible: Vec::new(),
            object_state: ListState::default(),
            status: None,
            command: None,
            creator: None,
            clipboard: None,
            help: None,
            confirm: None,
            marked: HashSet::new(),
            filter: None,
            filtering: false,
            rename: None,
            sort: SortKey::default(),
            sort_reverse: false,
            show_hidden: false,
        }
    }
}

impl App {
    /// Constructs a new instance of [`App`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Run the application's main loop.
    pub async fn run(mut self, mut terminal: DefaultTerminal) -> color_eyre::Result<()> {
        while self.running() {
            terminal.draw(|frame| {
                if let Some(creator) = &self.creator {
                    nimbus_creator::ui::render(creator, frame);
                } else {
                    frame.render_widget(&mut self, frame.area());
                }
            })?;
            self.handle_events(&mut terminal).await?;
        }
        Ok(())
    }

    pub async fn handle_events(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> color_eyre::Result<()> {
        match self.events.next()? {
            Event::Tick => self.tick(),
            Event::Crossterm(event) => match event {
                crossterm::event::Event::Key(key_event)
                    if key_event.kind == crossterm::event::KeyEventKind::Press =>
                {
                    self.handle_key_event(key_event, terminal).await?
                }
                _ => {}
            },
            Event::App(app_event) => self.apply_app_event(app_event).await,
        }
        Ok(())
    }

    /// Handles the key events and updates the state of [`App`].
    pub async fn handle_key_event(
        &mut self,
        key_event: KeyEvent,
        terminal: &mut DefaultTerminal,
    ) -> color_eyre::Result<()> {
        if let Some(creator) = self.creator.as_mut() {
            creator.handle_key_event(key_event)?;
            if !creator.is_running() {
                let outcome = self.creator.take().and_then(CreatorApp::into_outcome);
                self.finish_creator_wizard(outcome);
            }
            return Ok(());
        }

        if self.command.is_some() {
            self.handle_key_command(key_event).await;
            return Ok(());
        }

        if key_event.code == KeyCode::Char('c') && key_event.modifiers == KeyModifiers::CONTROL {
            self.events.send(AppEvent::Quit);
            return Ok(());
        }

        if self.filtering {
            self.handle_key_filter(key_event);
            return Ok(());
        }

        if self.rename.is_some() {
            self.handle_key_rename(key_event).await;
            return Ok(());
        }

        // Ahead of every other binding: while a confirmation is up, no key may mean what it
        // usually means, or a stray `d`/`p` would act on the list behind the question.
        if self.confirm.is_some() {
            self.handle_key_confirm(key_event).await;
            return Ok(());
        }

        if self.help.is_some() {
            self.handle_key_help(key_event);
            return Ok(());
        }

        match key_event.code {
            KeyCode::Char(':') => {
                self.command = Some(String::new());
                return Ok(());
            }
            KeyCode::Char('?') => {
                self.help = Some(0);
                return Ok(());
            }
            KeyCode::Char('n') => {
                self.apply_app_event(AppEvent::New { path: None }).await;
                return Ok(());
            }
            _ => {}
        }

        match &self.mode {
            AppMode::Root => self.handle_key_root(key_event).await,
            AppMode::Vault(_) => self.handle_key_vault(key_event, terminal).await,
            AppMode::New | AppMode::Quit => Ok(()),
        }
    }

    async fn handle_key_root(&mut self, key_event: KeyEvent) -> color_eyre::Result<()> {
        match key_event.code {
            KeyCode::Esc | KeyCode::Char('q') => self.events.send(AppEvent::Quit),
            KeyCode::Up | KeyCode::Char('k') => self.select_previous(Selector::Vault),
            KeyCode::Down | KeyCode::Char('j') => self.select_next(Selector::Vault),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if let Some(i) = self.vault_state.selected()
                    && let Some(name) = self.vaults.get(i).cloned()
                {
                    self.enter_vault(name).await;
                }
            }
            KeyCode::Char('x') | KeyCode::Delete => self.confirm_forget_vault(),
            KeyCode::Char('R') => {
                self.refresh_vaults();
                self.status = Some("refreshed".to_string());
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_key_vault(
        &mut self,
        key_event: KeyEvent,
        terminal: &mut DefaultTerminal,
    ) -> color_eyre::Result<()> {
        match key_event.code {
            KeyCode::Char('q') => self.events.send(AppEvent::Quit),
            KeyCode::Up | KeyCode::Char('k') => self.select_previous(Selector::Object),
            KeyCode::Down | KeyCode::Char('j') => self.select_next(Selector::Object),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if let Some(object) = self.selected_object().cloned() {
                    match object {
                        Object::Branch { name, .. } => self.descend(name).await,
                        Object::Leaf { name, id, .. } => self.open_object(name, id, terminal).await,
                        Object::Root { .. } => {}
                    }
                }
            }
            // Esc does double duty: an active filter is the more recent, more surprising state
            // to be in, so clear that first and only navigate once the full listing is back.
            KeyCode::Esc if self.filter.is_some() => self.clear_filter(),
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => {
                self.ascend().await;
            }
            KeyCode::Char('y') => self.yank(false),
            KeyCode::Char('d') => self.yank(true),
            KeyCode::Char('p') => self.paste().await,
            // `a`/`t` open the command line pre-filled rather than adding a prompt of their own:
            // a new object's name has to be typed from scratch either way, so there'd be nothing
            // for a dedicated prompt to offer. `r` is different — see `begin_rename`.
            KeyCode::Char('r') => self.begin_rename(),
            KeyCode::Char('a') => self.command = Some("mkdir ".to_string()),
            KeyCode::Char('t') => self.command = Some("touch ".to_string()),
            KeyCode::Char('x') | KeyCode::Delete => self.confirm_delete(),
            KeyCode::Char(' ') => self.toggle_mark(),
            KeyCode::Char('/') => {
                self.filtering = true;
                self.filter = Some(String::new());
                self.apply_view();
            }
            KeyCode::Char('s') => {
                self.sort = self.sort.next();
                self.apply_view();
                self.status = Some(self.sort_description());
            }
            KeyCode::Char('S') => {
                self.sort_reverse = !self.sort_reverse;
                self.apply_view();
                self.status = Some(self.sort_description());
            }
            KeyCode::Char('.') => {
                self.show_hidden = !self.show_hidden;
                self.apply_view();
                self.status = Some(if self.show_hidden {
                    "showing hidden objects".to_string()
                } else {
                    "hiding hidden objects".to_string()
                });
            }
            KeyCode::Char('R') => {
                self.refresh_objects().await;
                self.status = Some("refreshed".to_string());
            }
            _ => {}
        }
        Ok(())
    }

    fn selected_object_name(&self) -> Option<String> {
        self.selected_object().map(Object::get_name)
    }

    fn sort_description(&self) -> String {
        format!(
            "sort: {} {}",
            self.sort.label(),
            if self.sort_reverse { "desc" } else { "asc" }
        )
    }

    /// Types into the `/` filter. The listing narrows on every keystroke, so the result is
    /// visible before committing; Enter just stops typing and leaves the filter applied.
    fn handle_key_filter(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Esc => self.clear_filter(),
            KeyCode::Enter => {
                self.filtering = false;
                // An empty filter isn't a filter — leaving it set would show a pointless
                // `filter:` indicator that Esc then has to be spent clearing.
                if self.filter.as_ref().is_some_and(String::is_empty) {
                    self.filter = None;
                }
            }
            KeyCode::Backspace => {
                if let Some(filter) = self.filter.as_mut() {
                    filter.pop();
                }
                self.apply_view();
            }
            KeyCode::Char(c) => {
                if let Some(filter) = self.filter.as_mut() {
                    filter.push(c);
                }
                self.apply_view();
            }
            _ => {}
        }
    }

    /// Opens the rename prompt for the selected object, pre-filled with its current name.
    ///
    /// Unlike `a`/`t`, this gets a prompt of its own rather than pre-filling the `:` line: a
    /// rename is usually a small edit to a name that already exists, so starting from that name
    /// with it ready to edit is the whole convenience — typing `rename notes.txt ` and then the
    /// full new name spells out far more than the change is worth.
    fn begin_rename(&mut self) {
        let Some(original) = self.selected_object_name() else {
            self.status = Some("nothing selected".to_string());
            return;
        };
        self.rename = Some(RenamePrompt {
            input: original.clone(),
            original,
        });
    }

    /// Types into the rename prompt. Enter submits, Esc abandons it.
    async fn handle_key_rename(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Esc => {
                self.rename = None;
                self.status = None;
            }
            KeyCode::Enter => {
                let Some(prompt) = self.rename.take() else {
                    return;
                };
                let new_name = prompt.input.trim().to_string();
                // Submitting the name it already had isn't an error, it's a no-op — but core
                // would reject it as "already exists", which reads like a failure.
                if new_name.is_empty() || new_name == prompt.original {
                    self.status = None;
                    return;
                }
                let Some(vault) = self.nimbus.current_vault() else {
                    return;
                };
                // Fully qualified for the same reason a yank is: a name containing a colon
                // must not be read as a vault prefix.
                let path = format!(
                    "{}:{}",
                    vault,
                    PathBuf::from(self.nimbus.pwd())
                        .join(&prompt.original)
                        .display()
                );
                self.apply_app_event(AppEvent::Rename {
                    path,
                    new_name,
                    vault: None,
                })
                .await;
            }
            KeyCode::Backspace => {
                if let Some(prompt) = self.rename.as_mut() {
                    prompt.input.pop();
                }
            }
            KeyCode::Char(c) => {
                if let Some(prompt) = self.rename.as_mut() {
                    prompt.input.push(c);
                }
            }
            _ => {}
        }
    }

    fn clear_filter(&mut self) {
        self.filter = None;
        self.filtering = false;
        self.apply_view();
    }

    /// Arms a delete of the selected object, to run only once the user answers `y`.
    ///
    /// The object is addressed as a fully-qualified `vault:/path` for the same reason a yank is:
    /// it's what keeps a name containing a colon from being read as a vault prefix.
    fn confirm_delete(&mut self) {
        let Some(vault) = self.nimbus.current_vault() else {
            return;
        };
        let targets = self.targets();
        if targets.is_empty() {
            self.status = Some("nothing selected".to_string());
            return;
        }

        let prompt = match targets.as_slice() {
            [one] => match one {
                Object::Branch { name, .. } => {
                    format!("delete {name}/ and everything in it? (y/N)")
                }
                other => format!("delete {}? (y/N)", other.get_name()),
            },
            many => {
                let directories = many
                    .iter()
                    .filter(|o| matches!(o, Object::Branch { .. }))
                    .count();
                let scope = if directories > 0 {
                    format!(
                        " ({directories} director{} — contents included)",
                        if directories == 1 { "y" } else { "ies" }
                    )
                } else {
                    String::new()
                };
                format!("delete {} objects{scope}? (y/N)", many.len())
            }
        };

        let events = targets
            .iter()
            .map(|object| AppEvent::Delete {
                // Fully qualified for the same reason a yank is: it's what stops a name
                // containing a colon from being read as a vault prefix.
                path: format!(
                    "{}:{}",
                    vault,
                    PathBuf::from(self.nimbus.pwd())
                        .join(object.get_name())
                        .display()
                ),
                vault: None,
                // Forced: the prompt already says a directory takes its contents with it, so
                // having core refuse a second time would leave the user at a dead end with no
                // way to agree.
                force: true,
            })
            .collect();

        self.confirm = Some(Confirm { prompt, events });
    }

    /// Arms an unregister of the selected vault. Worth confirming even though no data is
    /// destroyed — re-registering means finding the config file again.
    fn confirm_forget_vault(&mut self) {
        let Some(name) = self
            .vault_state
            .selected()
            .and_then(|i| self.vaults.get(i))
            .cloned()
        else {
            return;
        };
        self.confirm = Some(Confirm {
            prompt: format!("stop tracking vault {name}? its files are left alone (y/N)"),
            events: vec![AppEvent::Forget { vault: name }],
        });
    }

    /// Answers a pending [`Confirm`]: `y` runs it, anything else drops it. Deliberately not
    /// keyed to Enter — the whole point is that the confirming key isn't one the user is
    /// already leaning on.
    async fn handle_key_confirm(&mut self, key_event: KeyEvent) {
        let Some(confirm) = self.confirm.take() else {
            return;
        };
        match key_event.code {
            KeyCode::Char('y' | 'Y') => {
                for event in confirm.events {
                    self.apply_app_event(event).await;
                }
            }
            _ => self.status = Some("cancelled".to_string()),
        }
    }

    fn handle_key_help(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Esc | KeyCode::Char('q' | '?') => self.help = None,
            KeyCode::Down | KeyCode::Char('j') => {
                self.help = self.help.map(|offset| offset.saturating_add(1));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.help = self.help.map(|offset| offset.saturating_sub(1));
            }
            _ => {}
        }
    }

    /// Picks up the selected object for a later `p`, recording it as a fully-qualified
    /// `vault:/path` so the paste still resolves after navigating elsewhere — including into a
    /// different vault, which is what makes cross-vault copy/move work from the keyboard.
    fn yank(&mut self, cut: bool) {
        let Some(vault) = self.nimbus.current_vault() else {
            return;
        };
        let targets = self.targets();
        if targets.is_empty() {
            return;
        }

        let entries: Vec<ClipboardEntry> = targets
            .iter()
            .map(|object| {
                let name = object.get_name();
                let path = PathBuf::from(self.nimbus.pwd()).join(&name);
                ClipboardEntry {
                    spec: format!("{}:{}", vault, path.display()),
                    name,
                }
            })
            .collect();

        let clipboard = Clipboard { entries, cut };
        self.status = Some(format!(
            "{} {} — press p to paste",
            if cut { "cut" } else { "yanked" },
            clipboard.label()
        ));
        self.clipboard = Some(clipboard);
        // The marks have done their job; leaving them set makes the next `x` or `y` act on a
        // selection the user has mentally moved on from.
        self.marked.clear();
    }

    /// Copies (or moves, for a cut) the clipboard's object into the directory currently being
    /// browsed, by handing both sides to `nimbus` as `vault:/path` specs — the same path syntax
    /// the `:cp`/`:mv` commands take, so both routes go through identical logic.
    async fn paste(&mut self) {
        let Some(clipboard) = &self.clipboard else {
            self.status = Some("nothing to paste".to_string());
            return;
        };
        let Some(vault) = self.nimbus.current_vault() else {
            self.status = Some("no vault selected".to_string());
            return;
        };

        let cut = clipboard.cut;
        let sources: Vec<String> = clipboard
            .entries
            .iter()
            .map(|entry| entry.spec.clone())
            .collect();
        let destination = format!("{}:{}", vault, self.nimbus.pwd());

        // Each object is pasted independently and the first failure stops the run, so a bad
        // entry (say, one deleted from under us) doesn't silently skip the rest.
        let mut result = Ok(());
        for source in sources {
            result = if cut {
                self.nimbus.mv(source, destination.clone(), None).await
            } else {
                self.nimbus.cp(source, destination.clone(), None).await
            };
            if result.is_err() {
                break;
            }
        }
        let succeeded = result.is_ok();

        self.report(result, if cut { "moved" } else { "copied" })
            .await;
        if succeeded && cut {
            self.clipboard = None;
        }
    }

    async fn handle_key_command(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Esc => {
                self.command = None;
                self.status = None;
            }
            KeyCode::Enter => {
                let line = self.command.take().unwrap_or_default();
                if line.trim().is_empty() {
                    return;
                }
                // Intercepted before clap, which would otherwise render its own multi-line help
                // as a parse *error* — unreadable in the single-line status bar.
                if line.trim() == "help" {
                    self.help = Some(0);
                    self.status = None;
                    return;
                }
                match command::parse(&line) {
                    Ok(event) => self.apply_app_event(event).await,
                    Err(msg) => self.status = Some(msg),
                }
            }
            KeyCode::Backspace => {
                if let Some(buffer) = self.command.as_mut() {
                    buffer.pop();
                }
            }
            KeyCode::Char(c) => {
                if let Some(buffer) = self.command.as_mut() {
                    buffer.push(c);
                }
            }
            _ => {}
        }
    }

    /// Executes a fully-parsed [`AppEvent`] against `nimbus`, whether it came from a keybinding
    /// (e.g. `q` sending `Quit`) or from the `:`-command line. Mirrors `nimbus-cli`'s `dispatch`.
    async fn apply_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Quit => self.quit(),
            AppEvent::Ls => {
                if matches!(self.mode, AppMode::Vault(_)) {
                    self.refresh_objects().await;
                } else {
                    self.refresh_vaults();
                }
            }
            AppEvent::Vaults => {
                self.mode = AppMode::Root;
                self.refresh_vaults();
            }
            AppEvent::Select { vault } => self.enter_vault(vault).await,
            AppEvent::New { path: Some(path) } => {
                match self.nimbus.new_vault(PathBuf::from(path)) {
                    Ok(()) => {
                        self.refresh_vaults();
                        self.status = Some("vault registered".to_string());
                    }
                    Err(e) => self.status = Some(e.to_string()),
                }
            }
            AppEvent::New { path: None } => self.creator = Some(CreatorApp::new()),
            AppEvent::Cd { path } => match self.nimbus.cd(path).await {
                Ok(()) => match self.nimbus.current_vault() {
                    Some(vault) => {
                        self.mode = AppMode::Vault(vault);
                        self.leave_directory();
                        self.refresh_objects().await;
                    }
                    None => {
                        self.mode = AppMode::Root;
                        self.leave_directory();
                        self.objects.clear();
                        self.visible.clear();
                        self.object_state = ListState::default();
                        self.refresh_vaults();
                    }
                },
                Err(e) => self.status = Some(e.to_string()),
            },
            AppEvent::Put { path, vault, dest } => {
                let result = self.nimbus.put(path, vault, dest).await;
                self.report(result, "put").await;
            }
            AppEvent::Get { path, vault, dest } => {
                let result = self.nimbus.get(path, vault, dest).await;
                self.report(result, "get").await;
            }
            AppEvent::Mkdir { path, vault } => {
                let result = self.nimbus.mkdir(path, vault).await;
                self.report(result, "created").await;
            }
            AppEvent::Touch { path, vault } => {
                let result = self.nimbus.touch(path, vault).await;
                self.report(result, "created").await;
            }
            AppEvent::Forget { vault } => match self.nimbus.forget_vault(vault) {
                Ok(()) => {
                    // Forgetting the vault being browsed drops us back to the root listing,
                    // since there's no longer anything under the cursor to be inside of.
                    if self.nimbus.current_vault().is_none() {
                        self.mode = AppMode::Root;
                        self.objects.clear();
                        self.visible.clear();
                        self.object_state = ListState::default();
                    }
                    self.refresh_vaults();
                    self.status = Some("vault forgotten".to_string());
                }
                Err(e) => self.status = Some(e.to_string()),
            },
            AppEvent::Rename {
                path,
                new_name,
                vault,
            } => {
                let result = self.nimbus.rename(path, new_name, vault).await;
                self.report(result, "renamed").await;
            }
            AppEvent::Delete { path, vault, force } => {
                let result = self.nimbus.delete(path, vault, force).await;
                self.report(result, "deleted").await;
            }
            AppEvent::Cp {
                path,
                destination,
                vault,
            } => {
                let result = self.nimbus.cp(path, destination, vault).await;
                self.report(result, "copied").await;
            }
            AppEvent::Mv {
                path,
                destination,
                vault,
            } => {
                let result = self.nimbus.mv(path, destination, vault).await;
                self.report(result, "moved").await;
            }
            AppEvent::Push { vault } => {
                let result = self.nimbus.push(vault).await;
                self.report(result, "pushed").await;
            }
            AppEvent::Pull { vault } => {
                let result = self.nimbus.pull(vault).await;
                self.report(result, "pulled").await;
            }
        }
    }

    /// Turns an `anyhow::Result<()>` from a `nimbus` mutation into a status message, and
    /// refreshes the object listing if it's currently visible (the mutation may have changed it).
    async fn report(&mut self, result: anyhow::Result<()>, verb: &str) {
        // Refresh first: `refresh_objects` sets `status` on its own (clearing it on success,
        // reporting its own error on failure), which would otherwise clobber the message below.
        if matches!(self.mode, AppMode::Vault(_)) {
            self.refresh_objects().await;
        }
        match result {
            Ok(()) => self.status = Some(format!("{verb} ok")),
            Err(e) => self.status = Some(e.to_string()),
        }
    }

    /// Registers the vault the creator wizard just wrote a config for, or does nothing if it
    /// was cancelled (`outcome` is `None`).
    fn finish_creator_wizard(&mut self, outcome: Option<PathBuf>) {
        let Some(path) = outcome else {
            return;
        };
        match self.nimbus.new_vault(path) {
            Ok(()) => {
                self.refresh_vaults();
                self.status = Some("vault created".to_string());
            }
            Err(e) => self.status = Some(e.to_string()),
        }
    }

    async fn enter_vault(&mut self, name: String) {
        if let Err(e) = self.nimbus.select(name.clone()) {
            self.status = Some(e.to_string());
            return;
        }
        self.mode = AppMode::Vault(name);
        self.leave_directory();
        self.refresh_objects().await;
    }

    async fn descend(&mut self, name: String) {
        if let Err(e) = self.nimbus.cd(Some(name)).await {
            self.status = Some(e.to_string());
            return;
        }
        self.leave_directory();
        self.refresh_objects().await;
    }

    /// Drops the per-directory view state on the way out. Marks name objects that only exist
    /// here, and a filter left applied silently hides things in the directory you land in.
    fn leave_directory(&mut self) {
        self.marked.clear();
        self.filter = None;
        self.filtering = false;
    }

    /// Fetches a `Leaf`'s bytes into a temp file and opens it: first with the OS's default
    /// file-association opener (`open`/`xdg-open`/`start`), falling back to `$EDITOR` (or a
    /// platform-sensible default) if there's no working association for the file type. Once the
    /// program exits, any edit it made to the temp copy is written back to the object's origin.
    async fn open_object(&mut self, name: String, id: ObjectId, terminal: &mut DefaultTerminal) {
        let bytes = match self.nimbus.fetch_object_bytes(id.clone()).await {
            Ok(bytes) => bytes,
            Err(e) => {
                self.status = Some(e.to_string());
                return;
            }
        };

        let dir = std::env::temp_dir().join(format!("nimbus-tui-{}", std::process::id()));
        let path = dir.join(&name);
        if let Err(e) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, &bytes)) {
            self.status = Some(e.to_string());
            return;
        }

        // Neither branch below is guaranteed to detach from the terminal: a desktop entry can
        // have `Terminal=true` (common for CLI tools — nvim's own .desktop ships with it) and
        // without a terminal-emulator wrapper available, `xdg-open` et al. fall back to execing
        // it attached straight to our controlling tty, same as `$EDITOR` always does. So both
        // attempts get the same treatment: suspend our own input thread and release raw
        // mode/alt screen first, and only reclaim them once whichever process has exited.
        self.events.suspend();
        ratatui::restore();

        let opened = crate::opener::try_os_open(&path);
        let failure = if opened {
            None
        } else {
            match crate::opener::editor_command().arg(&path).status() {
                Ok(status) if status.success() => None,
                Ok(status) => Some(format!("editor exited with {status}")),
                Err(e) => Some(e.to_string()),
            }
        };

        *terminal = ratatui::init();
        self.events.resume();

        if let Some(failure) = failure {
            self.status = Some(failure);
            return;
        }
        self.status = self.write_back(&name, id, &path, &bytes, opened).await;
    }

    /// Saves the temp copy at `path` back to the object it came from, if whatever opened it
    /// actually changed the bytes. Returns the status message to show.
    ///
    /// Detecting the change by content (rather than always uploading) keeps a plain "look at
    /// this file" from rewriting it — which for some origins would be a real write with a new
    /// modification time, enough to make the next `push`/`pull` think it diverged.
    async fn write_back(
        &mut self,
        name: &str,
        id: ObjectId,
        path: &std::path::Path,
        original: &[u8],
        os_opened: bool,
    ) -> Option<String> {
        let updated = match std::fs::read(path) {
            Ok(updated) => updated,
            Err(e) => return Some(format!("couldn't read back {name}: {e}")),
        };

        if updated == original {
            // `xdg-open`/`open` usually hand off to a GUI app and return immediately, so an
            // unchanged file here doesn't mean the user is done with it — say so rather than
            // implying the edit was captured.
            return Some(if os_opened {
                format!("opened {name} — edits made after this won't be saved back")
            } else {
                format!("closed {name} — unchanged")
            });
        }

        match self.nimbus.write_object_bytes(id, updated).await {
            Ok(()) => {
                self.refresh_objects().await;
                Some(format!("saved {name}"))
            }
            Err(e) => Some(format!("couldn't save {name}: {e}")),
        }
    }

    async fn ascend(&mut self) {
        if self.nimbus.pwd() == "/" {
            if let Err(e) = self.nimbus.cd(None).await {
                self.status = Some(e.to_string());
                return;
            }
            self.mode = AppMode::Root;
            self.leave_directory();
            self.objects.clear();
            self.visible.clear();
            self.object_state = ListState::default();
            return;
        }
        if let Err(e) = self.nimbus.cd(Some("..".to_string())).await {
            self.status = Some(e.to_string());
            return;
        }
        self.leave_directory();
        self.refresh_objects().await;
    }

    fn refresh_vaults(&mut self) {
        self.vaults = self.nimbus.vault_names();
        if self
            .vault_state
            .selected()
            .is_none_or(|i| i >= self.vaults.len())
        {
            self.vault_state
                .select((!self.vaults.is_empty()).then_some(0));
        }
    }

    async fn refresh_objects(&mut self) {
        match self.nimbus.list_cwd().await {
            Ok(objects) => {
                self.objects = objects;
                // Anything marked that the refresh removed (deleted, moved, renamed) would
                // otherwise stay marked forever and be silently included in the next bulk op.
                let present: HashSet<String> = self.objects.iter().map(Object::get_name).collect();
                self.marked.retain(|name| present.contains(name));
                self.apply_view();
                self.status = None;
            }
            Err(e) => self.status = Some(e.to_string()),
        }
    }

    /// Recomputes [`Self::visible`] from `objects` — dropping what the filter and hidden-file
    /// setting exclude, then ordering by the current sort — and keeps the cursor on the same
    /// object across the change where it still exists.
    pub fn apply_view(&mut self) {
        let previously_selected = self.selected_object().map(Object::get_name);
        let needle = self.filter.as_ref().map(|f| f.to_lowercase());

        let mut visible: Vec<usize> = (0..self.objects.len())
            .filter(|&i| {
                let name = self.objects[i].get_name();
                if !self.show_hidden && name.starts_with('.') {
                    return false;
                }
                match &needle {
                    Some(needle) => name.to_lowercase().contains(needle.as_str()),
                    None => true,
                }
            })
            .collect();

        visible.sort_by(|&a, &b| {
            let (a, b) = (&self.objects[a], &self.objects[b]);
            // Directories first, always — and never reversed, so `S` flips the ordering within
            // each group rather than burying every directory at the bottom.
            match (a, b) {
                (Object::Branch { .. }, Object::Leaf { .. }) => return std::cmp::Ordering::Less,
                (Object::Leaf { .. }, Object::Branch { .. }) => return std::cmp::Ordering::Greater,
                _ => {}
            }
            let ordering = match self.sort {
                SortKey::Name => std::cmp::Ordering::Equal,
                SortKey::Size => meta_size(a).cmp(&meta_size(b)),
                SortKey::Modified => meta_modified(a).cmp(&meta_modified(b)),
            };
            // Name is the tie-break for every key, so equal sizes/timestamps still come out in
            // a stable, readable order rather than whatever the origin happened to list.
            let ordering = ordering.then_with(|| a.get_name().cmp(&b.get_name()));
            if self.sort_reverse {
                ordering.reverse()
            } else {
                ordering
            }
        });

        self.visible = visible;

        let restored = previously_selected.and_then(|name| {
            self.visible
                .iter()
                .position(|&i| self.objects[i].get_name() == name)
        });
        self.object_state = ListState::default();
        self.object_state.select(match restored {
            Some(index) => Some(index),
            None if self.visible.is_empty() => None,
            None => Some(0),
        });
    }

    /// The object under the cursor, if any.
    pub fn selected_object(&self) -> Option<&Object> {
        let index = self.object_state.selected()?;
        self.objects.get(*self.visible.get(index)?)
    }

    /// What a bulk operation should act on: everything marked, or the cursor when nothing is.
    fn targets(&self) -> Vec<Object> {
        if self.marked.is_empty() {
            return self.selected_object().cloned().into_iter().collect();
        }
        // Taken in display order rather than `marked`'s (a `HashSet`, so arbitrary), which is
        // what makes a multi-object status message match what's on screen.
        self.visible
            .iter()
            .map(|&i| &self.objects[i])
            .filter(|object| self.marked.contains(&object.get_name()))
            .cloned()
            .collect()
    }

    /// Marks or unmarks the object under the cursor and steps down, so a run of objects can be
    /// selected by holding space.
    fn toggle_mark(&mut self) {
        let Some(name) = self.selected_object().map(Object::get_name) else {
            return;
        };
        if !self.marked.remove(&name) {
            self.marked.insert(name);
        }
        self.select_next(Selector::Object);
    }

    fn select_previous(&mut self, selector: Selector) {
        let (state, len) = self.selector_parts(selector);
        if len == 0 {
            return;
        }
        let i = match state.selected() {
            Some(0) | None => len - 1,
            Some(i) => i - 1,
        };
        state.select(Some(i));
    }

    fn select_next(&mut self, selector: Selector) {
        let (state, len) = self.selector_parts(selector);
        if len == 0 {
            return;
        }
        let i = match state.selected() {
            Some(i) if i + 1 < len => i + 1,
            _ => 0,
        };
        state.select(Some(i));
    }

    fn selector_parts(&mut self, selector: Selector) -> (&mut ListState, usize) {
        match selector {
            Selector::Vault => (&mut self.vault_state, self.vaults.len()),
            Selector::Object => (&mut self.object_state, self.visible.len()),
        }
    }

    /// Handles the tick event of the terminal.
    pub fn tick(&self) {}

    /// Set running to false to quit the application.
    pub fn quit(&mut self) {
        self.mode = AppMode::Quit;
    }
}

#[derive(Clone, Copy)]
enum Selector {
    Vault,
    Object,
}

/// An object's size, for sorting. Directories report whatever the origin gave them, which is
/// rarely meaningful, so they're pinned to 0 — they're grouped separately anyway.
fn meta_size(object: &Object) -> u64 {
    match object {
        Object::Branch { .. } | Object::Root { .. } => 0,
        _ => object.get_meta().and_then(|meta| meta.size).unwrap_or(0),
    }
}

/// An object's modification time, for sorting. Objects whose origin doesn't report one sort as
/// oldest rather than being dropped from the ordering.
fn meta_modified(object: &Object) -> chrono::DateTime<chrono::Utc> {
    object
        .get_meta()
        .and_then(|meta| meta.modified)
        .unwrap_or(chrono::DateTime::UNIX_EPOCH)
}
