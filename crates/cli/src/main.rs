use clap::{Command, arg};

use crate::state::App;

mod state;
/*
    Object: file or folder
    List Of Commands:
        ls //if Root list all vaults, if in a vault, list vault contents
        new <VAULT NAME> //creates a new vault
        cd <PATH> changes the current working directory to a vault or an object inside a vault
        put <PATH> <OPTIONAL: as [NAME]> //puts an object onto the vault
        get <NAME> <OPTIONAL: [DESTINATION PATH]> //gets a object from vault if path is not specified, path is the current directory from where nimbus was called
        cp <NAME> <NAME>: // copies a object inside the same vault
        mv <NAME> <NAME>: //moves a object inside the same vault
        origin <VAULT NAME> <ORIGIN> //sets the origin of the vault either the file system, a db or a remote origin
        sync //syncs vault with his origin





*/

const AUTHOR: &str = "PeachGB";
const VERSION: &str = "0.1.0";
const ABOUT: &str = "A CLI tool for managing and syncing Objects in vaults.\n\n\
A vault is a tree like structure like a filesystem \n\
Vaults store metadata and a way to get the object. \n\
You can make vaults out of APIs, your own filesystem, etc";

#[tokio::main]
async fn main() {
    let nimbus = Command::new("Nimbus")
        .version(VERSION)
        .author(AUTHOR)
        .about(ABOUT)
        .subcommand(
            Command::new("ls")
                .about("Lists all vaults or contents of a vault")
                .arg(arg!(<VAULT_NAME> "Optional vault name to list contents")),
        )
        .subcommand(
            Command::new("new")
                .about("Creates a new vault")
                .arg(arg!(<PATH> "Path to the new vault").required(true)),
        )
        .subcommand(
            Command::new("cd")
                .about(
                    "Changes the current working directory to a vault or an object inside a vault",
                )
                .arg(arg!(<PATH> "Path to the vault or object")),
        )
        .subcommand(
            Command::new("put")
                .about("Puts an object onto the vault")
                .arg(arg!(<PATH> "Path to the object"))
                .arg(arg!([NAME] "Optional name for the object in the vault")),
        )
        .subcommand(
            Command::new("get")
                .about("Gets an object from the vault")
                .arg(arg!(<NAME> "Name of the object to get"))
                .arg(arg!([DESTINATION_PATH] "Optional destination path")),
        )
        .subcommand(
            Command::new("cp")
                .about("Copies an object inside the same vault")
                .arg(arg!(<NAME> "Name of the object to copy"))
                .arg(arg!(<DESTINATION_NAME> "Name of the destination object")),
        )
        .subcommand(
            Command::new("mv")
                .about("Moves an object inside the same vault")
                .arg(arg!(<NAME> "Name of the object to move"))
                .arg(arg!(<DESTINATION_NAME> "Name of the destination object")),
        )
        .subcommand(
            Command::new("origin")
                .about("Sets the origin of the vault")
                .arg(arg!(<VAULT_NAME> "Name of the vault").required(true))
                .arg(arg!(<ORIGIN> "Origin to set (file system, db, remote)").required(true)),
        )
        .subcommand(Command::new("sync").about("Syncs the vault contents with his origin"))
        .get_matches();

    let app = App::init();
    match nimbus.subcommand() {
        Some(("ls", sub_matches)) => app.ls(),
        Some(("new", sub_matches)) => app.new(),
        Some(("cd", sub_matches)) => app.cd(),
        Some(("put", sub_matches)) => app.put(),
        Some(("get", sub_matches)) => app.get(),
        Some(("cp", sub_matches)) => app.cp(),
        Some(("mv", sub_matches)) => app.mv(),
        Some(("origin", sub_matches)) => app.origin(),
        Some(("sync", sub_matches)) => app.sync(),
        Some((_, _)) => todo!(),
        None => todo!(),
        _ => todo!(),
    }
}
