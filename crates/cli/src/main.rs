use clap::{Parser, Subcommand};

use crate::completion::NimbusHelper;
use anyhow::{Result, anyhow};
use nimbus_core::app::App;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

pub mod completion;
mod offline;

#[derive(Parser)]
#[command(version,long_about=None)]
#[command(propagate_version = true)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    ///list directory objects, if in root, list all available vaults
    Ls,
    ///lists vaults
    Vaults,
    Select {
        vault: String,
    },
    ///registers a vault; without a path, launches an interactive wizard to build one
    New {
        path: Option<String>,
    },
    ///changes current working directory, if in root, selects working vault
    Cd {
        path: Option<String>,
    },
    ///puts file inside vault
    Put {
        path: String,
        vault: Option<String>,
        dest: Option<String>,
        /// overwrite an existing object of the same name
        #[arg(short, long)]
        force: bool,
    },
    ///gets object inside vault; without a destination it lands in the local vault's root
    Get {
        path: String,
        vault: Option<String>,
        dest: Option<String>,
        /// overwrite an existing object of the same name
        #[arg(short, long)]
        force: bool,
    },
    ///creates a directory
    Mkdir {
        path: String,
        vault: Option<String>,
    },
    ///creates an empty file; refuses to overwrite an existing one
    Touch {
        path: String,
        vault: Option<String>,
    },
    ///stops tracking a vault; its config file and data are left alone
    Forget {
        vault: String,
    },
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
        /// overwrite an existing object of the same name
        #[arg(short, long)]
        force: bool,
    },
    ///moves an object; paths are relative to the cwd, or prefix one with `vault:` to address
    ///another vault from its root. The destination may be an existing directory, or a new
    ///path to move it under that name (e.g. `mv notes.txt backup:/inbox/copy.txt`)
    Mv {
        path: String,
        destination: String,
        vault: Option<String>,
        /// overwrite an existing object of the same name
        #[arg(short, long)]
        force: bool,
    },
    ///push local vault into vault; files newer on the vault are reported, not overwritten
    Push {
        vault: Option<String>,
        /// also overwrite files that are newer on the destination
        #[arg(short, long)]
        force: bool,
    },
    ///pulls from origin onto local vault; files newer locally are reported, not overwritten
    Pull {
        vault: Option<String>,
        /// also overwrite files that are newer on the destination
        #[arg(short, long)]
        force: bool,
    },
    ///writes a new encryption key for a crypt vault into a new file (never overwrites one).
    ///Losing the key loses the data encrypted with it
    Keygen {
        path: PathBuf,
    },
    ///decrypts one stored object straight from its file, without a vault: the recovery tool.
    ///Writes nothing unless the whole object checks out
    Decrypt {
        input: PathBuf,
        output: PathBuf,
        /// file holding the key (as written by `keygen`)
        #[arg(
            long,
            value_name = "FILE",
            conflicts_with = "key_env",
            required_unless_present = "key_env"
        )]
        key_file: Option<PathBuf>,
        /// environment variable holding the key
        #[arg(long, value_name = "VAR")]
        key_env: Option<String>,
        /// replace an existing output file
        #[arg(short, long)]
        force: bool,
    },
    Exit,
}

impl Commands {
    /// Whether the command works on vaults, so needs the app (registry, session, config)
    /// loaded first. `keygen` and `decrypt` don't: they must work even when that can't load.
    fn needs_app(&self) -> bool {
        !matches!(self, Commands::Keygen { .. } | Commands::Decrypt { .. })
    }
}

/// Runs a command that needs no app (see [`Commands::needs_app`]).
async fn run_offline(command: Commands) -> Result<()> {
    match command {
        Commands::Keygen { path } => offline::keygen(&path),
        Commands::Decrypt {
            input,
            output,
            key_file,
            key_env,
            force,
        } => {
            offline::decrypt(
                &input,
                &output,
                key_file.as_deref(),
                key_env.as_deref(),
                force,
            )
            .await
        }
        _ => unreachable!("only commands that don't need the app run offline"),
    }
}

// Never runs concurrently with the completer's borrow (which only borrows synchronously
// inside `readline()`, already returned by the time this is called).
#[allow(clippy::await_holding_refcell_ref)]
async fn run_dispatch(app: &Rc<RefCell<App>>, cli: Cli) -> Result<()> {
    let result = dispatch(&mut app.borrow_mut(), cli).await;
    print_warnings(&mut app.borrow_mut());
    result
}

/// Prints, to stderr, the warnings the core gathered since they were last taken.
fn print_warnings(app: &mut App) {
    write_warnings(&mut std::io::stderr(), &app.take_warnings());
}

/// Writes `warnings` one per line. Each already says whether it's a WARNING or a NOTE.
fn write_warnings(out: &mut impl std::io::Write, warnings: &[String]) {
    for warning in warnings {
        let _ = writeln!(out, "{warning}");
    }
}

/// Writes the notice for a key file the creator wizard wrote and kept without saving a config,
/// if there is one, so the user knows the file exists and what it's for.
fn write_unsaved_key_notice(out: &mut impl std::io::Write, outcome: &nimbus_creator::Outcome) {
    if let Some(key_file) = &outcome.unsaved_key_file {
        let _ = writeln!(out, "{}", nimbus_creator::kept_key_notice(key_file));
    }
}

/// Prints a push/pull summary. Conflicts aren't an error (everything else was synced), but they
/// are what the user most needs to see, so they go to stderr with a hint.
fn print_sync(report: nimbus_core::SyncReport) -> Result<()> {
    match report.conflicts.is_empty() {
        true => println!("{}", sync_summary(&report)),
        false => eprintln!("{}", sync_summary(&report)),
    }
    Ok(())
}

/// The push/pull summary: counts, one line per conflict with its reason, and the `--force` hint
/// only when some conflict can actually be forced. A file/directory mismatch or a symlink
/// leaving the vault can't be, so suggesting it for those would send the user in circles.
fn sync_summary(report: &nimbus_core::SyncReport) -> String {
    match report.has_forceable_conflicts() {
        true => format!(
            "{report}\nfiles newer at the destination were left as they are; rerun with --force to \
             overwrite them"
        ),
        false => report.to_string(),
    }
}

async fn dispatch(app: &mut App, cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Ls => app.ls().await,
        Commands::Vaults => app.vaults(),
        Commands::Select { vault } => app.select(vault),
        Commands::New { path: Some(path) } => app.new_vault(PathBuf::from(path)),
        Commands::New { path: None } => {
            let mut terminal = ratatui::init();
            let result = nimbus_creator::run_for_outcome(&mut terminal);
            ratatui::restore();
            let outcome = result.map_err(|e| anyhow!(e.to_string()))?;
            // After the restore: printed inside the alternate screen, it would vanish with it.
            write_unsaved_key_notice(&mut std::io::stderr(), &outcome);
            match outcome.config {
                Some(path) => app.new_vault(path),
                None => Ok(()),
            }
        }
        Commands::Cd { path } => app.cd(path).await,
        Commands::Put {
            path,
            vault,
            dest,
            force,
        } => app.put_with(path, vault, dest, force).await,
        Commands::Get {
            path,
            vault,
            dest,
            force,
        } => app.get_with(path, vault, dest, force).await,
        Commands::Cp {
            path,
            destination,
            vault,
            force,
        } => app.cp_with(path, destination, vault, force).await,
        Commands::Mv {
            path,
            destination,
            vault,
            force,
        } => app.mv_with(path, destination, vault, force).await,
        Commands::Mkdir { path, vault } => app.mkdir(path, vault).await,
        Commands::Touch { path, vault } => app.touch(path, vault).await,
        Commands::Forget { vault } => app.forget_vault(vault),
        Commands::Rename {
            path,
            new_name,
            vault,
        } => app.rename(path, new_name, vault).await,
        Commands::Delete { path, vault, force } => app.delete(path, vault, force).await,
        Commands::Push { vault, force } => print_sync(app.push_with(vault, force).await?),
        Commands::Pull { vault, force } => print_sync(app.pull_with(vault, force).await?),
        Commands::Exit => app.exit(),
        command @ (Commands::Keygen { .. } | Commands::Decrypt { .. }) => {
            run_offline(command).await
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Anything after the program name is a one-shot invocation (`nimbus ls`, `nimbus put a.txt`):
    // run it and exit. The REPL is only for a bare `nimbus`. Parsed before the app loads, so
    // `keygen`/`decrypt` run even when the config or session can't.
    let one_shot = std::env::args_os().nth(1).is_some().then(Cli::parse);
    if let Some(cli) = one_shot.as_ref()
        && !cli.command.needs_app()
    {
        let cli = one_shot.expect("checked above");
        return run_offline(cli.command).await;
    }

    // Restored before the `RefCell`, so the borrow isn't held across the await.
    let mut app = App::init()?;
    app.restore_session().await?;
    print_warnings(&mut app);
    let app = Rc::new(RefCell::new(app));

    if let Some(cli) = one_shot {
        let result = run_dispatch(&app, cli).await;
        app.borrow().save()?;
        return result;
    }

    repl(&app).await?;

    app.borrow().save()?;
    Ok(())
}

async fn repl(app: &Rc<RefCell<App>>) -> Result<()> {
    let mut rl: rustyline::Editor<NimbusHelper, rustyline::history::DefaultHistory> =
        rustyline::Editor::new()?;
    rl.set_helper(Some(NimbusHelper::new(app.clone())));
    loop {
        let cwd = {
            let app = app.borrow();
            match app.current_vault() {
                Some(vault) => format!("{}{}", vault, app.pwd()),
                None => app.pwd(),
            }
        };
        match rl.readline(&format!("nimbus {}>>", cwd)) {
            Ok(line) => {
                rl.add_history_entry(line.as_str())?;
                let tokens = std::iter::once("nimbus".to_string())
                    .chain(line.split_whitespace().map(String::from));
                match Cli::try_parse_from(tokens) {
                    Ok(cli) => {
                        if let Err(e) = run_dispatch(app, cli).await {
                            eprintln!("Error {e}");
                        }
                    }
                    Err(e) => {
                        e.print().ok();
                    }
                }
            }
            Err(rustyline::error::ReadlineError::Interrupted)
            | Err(rustyline::error::ReadlineError::Eof) => break,
            Err(e) => {
                println!("{e}");
                break;
            }
        }
    }

    app.borrow().save()?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/main.rs"]
mod tests;
