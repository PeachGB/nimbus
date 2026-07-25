use clap::{Parser, Subcommand};

use crate::completion::NimbusHelper;
use anyhow::{Result, anyhow};
use nimbus_core::app::App;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

pub mod completion;

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
    },
    ///gets object inside vault; without a destination it lands in the local vault's root
    Get {
        path: String,
        vault: Option<String>,
        dest: Option<String>,
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
    Push {
        vault: Option<String>,
    },
    ///pulls from origin onto local vault
    Pull {
        vault: Option<String>,
    },
    Exit,
}

// Never runs concurrently with the completer's borrow (which only borrows synchronously
// inside `readline()`, already returned by the time this is called).
#[allow(clippy::await_holding_refcell_ref)]
async fn run_dispatch(app: &Rc<RefCell<App>>, cli: Cli) -> Result<()> {
    dispatch(&mut app.borrow_mut(), cli).await
}

async fn dispatch(app: &mut App, cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Ls => app.ls().await,
        Commands::Vaults => app.vaults(),
        Commands::Select { vault } => app.select(vault),
        Commands::New { path: Some(path) } => app.new_vault(PathBuf::from(path)),
        Commands::New { path: None } => {
            let mut terminal = ratatui::init();
            let result = nimbus_creator::run(&mut terminal);
            ratatui::restore();
            match result.map_err(|e| anyhow!(e.to_string()))? {
                Some(path) => app.new_vault(path),
                None => Ok(()),
            }
        }
        Commands::Cd { path } => app.cd(path).await,
        Commands::Put { path, vault, dest } => app.put(path, vault, dest).await,
        Commands::Get { path, vault, dest } => app.get(path, vault, dest).await,
        Commands::Cp {
            path,
            destination,
            vault,
        } => app.cp(path, destination, vault).await,
        Commands::Mv {
            path,
            destination,
            vault,
        } => app.mv(path, destination, vault).await,
        Commands::Mkdir { path, vault } => app.mkdir(path, vault).await,
        Commands::Touch { path, vault } => app.touch(path, vault).await,
        Commands::Forget { vault } => app.forget_vault(vault),
        Commands::Rename {
            path,
            new_name,
            vault,
        } => app.rename(path, new_name, vault).await,
        Commands::Delete { path, vault, force } => app.delete(path, vault, force).await,
        Commands::Push { vault } => app.push(vault).await,
        Commands::Pull { vault } => app.pull(vault).await,
        Commands::Exit => app.exit(),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let app = Rc::new(RefCell::new(App::init()?));

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
                        if let Err(e) = run_dispatch(&app, cli).await {
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
