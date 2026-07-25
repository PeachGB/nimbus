use color_eyre::eyre::WrapErr;
use crossterm::event::{self, Event as CrosstermEvent};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

/// The frequency at which tick events are emitted.
const TICK_FPS: f64 = 30.0;

/// Representation of all possible events.
#[derive(Clone, Debug)]
pub enum Event {
    /// An event that is emitted on a regular schedule.
    ///
    /// Use this event to run any code which has to run outside of being a direct response to a user
    /// event. e.g. polling exernal systems, updating animations, or rendering the UI based on a
    /// fixed frame rate.
    Tick,
    /// Crossterm events.
    ///
    /// These events are emitted by the terminal.
    Crossterm(CrosstermEvent),
    /// Application events.
    ///
    /// Use this event to emit custom events that are specific to your application.
    App(AppEvent),
}

/// Application events.
///
/// Doubles as the grammar for the TUI's `:`-prefixed command line (see `crate::command`),
/// which parses a typed line straight into one of these variants via `clap::Subcommand` —
/// mirroring `nimbus-cli`'s `Commands` enum so the TUI exposes the same operations.
#[derive(Clone, Debug, clap::Subcommand)]
pub enum AppEvent {
    ///list directory objects, if in root, list all available vaults
    Ls,
    ///lists vaults
    Vaults,
    ///makes a vault the working vault, at its root
    Select { vault: String },
    ///registers a vault; without a path, launches an interactive wizard to build one
    New { path: Option<String> },
    ///changes current working directory, if in root, selects working vault
    Cd { path: Option<String> },
    ///puts file inside vault
    Put {
        path: String,
        vault: Option<String>,
        dest: Option<String>,
    },
    ///gets object inside vault; without a destination it lands in the local vault's root
    Get {
        path: String,
        vault: Option<String>,
        dest: Option<String>,
    },
    ///creates a directory
    Mkdir { path: String, vault: Option<String> },
    ///creates an empty file; refuses to overwrite an existing one
    Touch { path: String, vault: Option<String> },
    ///stops tracking a vault; its config file and data are left alone
    Forget { vault: String },
    ///renames an object in place; takes a name, not a path (use mv to relocate)
    Rename {
        path: String,
        new_name: String,
        vault: Option<String>,
    },
    ///deletes an object; a non-empty directory needs --force. Paths are relative to the cwd,
    ///or prefix one with `vault:` to address another vault from its root
    Delete {
        path: String,
        vault: Option<String>,
        #[arg(short, long)]
        force: bool,
    },
    ///copies an object; paths are relative to the cwd, or prefix one with `vault:` to address
    ///another vault from its root. The destination may be an existing directory, or a new
    ///path to copy it under that name (e.g. `cp notes.txt backup:/inbox/copy.txt`)
    Cp {
        path: String,
        destination: String,
        vault: Option<String>,
    },
    ///moves an object; paths are relative to the cwd, or prefix one with `vault:` to address
    ///another vault from its root. The destination may be an existing directory, or a new
    ///path to move it under that name (e.g. `mv notes.txt backup:/inbox/copy.txt`)
    Mv {
        path: String,
        destination: String,
        vault: Option<String>,
    },
    ///push local vault into vault
    Push { vault: Option<String> },
    ///pulls from origin onto local vault
    Pull { vault: Option<String> },
    /// Quit the application.
    #[command(visible_alias = "exit")]
    Quit,
}

/// Terminal event handler.
#[derive(Debug)]
pub struct EventHandler {
    /// Event sender channel.
    sender: mpsc::Sender<Event>,
    /// Event receiver channel.
    receiver: mpsc::Receiver<Event>,
    /// Set while an external child process (e.g. a spawned `$EDITOR`) has been handed the real
    /// terminal, so this handler's thread stops reading crossterm events until it's given back —
    /// otherwise the two would race each other for the same stdin.
    suspended: Arc<AtomicBool>,
}

impl EventHandler {
    /// Constructs a new instance of [`EventHandler`] and spawns a new thread to handle events.
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        let suspended = Arc::new(AtomicBool::new(false));
        let actor = EventThread::new(sender.clone(), suspended.clone());
        thread::spawn(|| actor.run());
        Self {
            sender,
            receiver,
            suspended,
        }
    }

    /// Stops this handler's background thread from reading crossterm events, so a foreground
    /// child process that's about to inherit our stdio (e.g. `$EDITOR`) has exclusive access to
    /// it. Call [`Self::resume`] once it exits and the terminal has been reclaimed.
    pub fn suspend(&self) {
        self.suspended.store(true, Ordering::SeqCst);
    }

    /// Resumes crossterm event reading after [`Self::suspend`].
    pub fn resume(&self) {
        self.suspended.store(false, Ordering::SeqCst);
    }

    /// Receives an event from the sender.
    ///
    /// This function blocks until an event is received.
    ///
    /// # Errors
    ///
    /// This function returns an error if the sender channel is disconnected. This can happen if an
    /// error occurs in the event thread. In practice, this should not happen unless there is a
    /// problem with the underlying terminal.
    pub fn next(&self) -> color_eyre::Result<Event> {
        Ok(self.receiver.recv()?)
    }

    /// Queue an app event to be sent to the event receiver.
    ///
    /// This is useful for sending events to the event handler which will be processed by the next
    /// iteration of the application's event loop.
    pub fn send(&mut self, app_event: AppEvent) {
        // Ignore the result as the reciever cannot be dropped while this struct still has a
        // reference to it
        let _ = self.sender.send(Event::App(app_event));
    }
}

/// A thread that handles reading crossterm events and emitting tick events on a regular schedule.
struct EventThread {
    /// Event sender channel.
    sender: mpsc::Sender<Event>,
    /// Mirrors [`EventHandler::suspended`]; while set, this thread neither polls nor reads
    /// crossterm events.
    suspended: Arc<AtomicBool>,
}

impl EventThread {
    /// Constructs a new instance of [`EventThread`].
    fn new(sender: mpsc::Sender<Event>, suspended: Arc<AtomicBool>) -> Self {
        Self { sender, suspended }
    }

    /// Runs the event thread.
    ///
    /// This function emits tick events at a fixed rate and polls for crossterm events in between.
    fn run(self) -> color_eyre::Result<()> {
        let tick_interval = Duration::from_secs_f64(1.0 / TICK_FPS);
        let mut last_tick = Instant::now();
        loop {
            if self.suspended.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(50));
                continue;
            }
            // emit tick events at a fixed rate
            let timeout = tick_interval.saturating_sub(last_tick.elapsed());
            if timeout == Duration::ZERO {
                last_tick = Instant::now();
                self.send(Event::Tick);
            }
            // poll for crossterm events, ensuring that we don't block the tick interval
            if event::poll(timeout).wrap_err("failed to poll for crossterm events")? {
                let event = event::read().wrap_err("failed to read crossterm event")?;
                self.send(Event::Crossterm(event));
            }
        }
    }

    /// Sends an event to the receiver.
    fn send(&self, event: Event) {
        // Ignores the result because shutting down the app drops the receiver, which causes the send
        // operation to fail. This is expected behavior and should not panic.
        let _ = self.sender.send(event);
    }
}
