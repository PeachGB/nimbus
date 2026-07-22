use super::*;

fn parse(args: &[&str]) -> Commands {
    let mut full = vec!["nimbus"];
    full.extend_from_slice(args);
    Cli::try_parse_from(full).unwrap().command
}

#[test]
fn parses_ls_and_vaults() {
    assert!(matches!(parse(&["ls"]), Commands::Ls));
    assert!(matches!(parse(&["vaults"]), Commands::Vaults));
}

#[test]
fn parses_select() {
    match parse(&["select", "myvault"]) {
        Commands::Select { vault } => assert_eq!(vault, "myvault"),
        _ => panic!("expected Select"),
    }
}

#[test]
fn parses_new() {
    match parse(&["new", "vault.toml"]) {
        Commands::New { path } => assert_eq!(path, Some("vault.toml".to_string())),
        _ => panic!("expected New"),
    }
}

#[test]
fn parses_new_with_no_path() {
    match parse(&["new"]) {
        Commands::New { path } => assert_eq!(path, None),
        _ => panic!("expected New"),
    }
}

#[test]
fn parses_cd() {
    match parse(&["cd", "docs"]) {
        Commands::Cd { path } => assert_eq!(path, Some("docs".to_string())),
        _ => panic!("expected Cd"),
    }
}

#[test]
fn parses_cd_with_no_path() {
    match parse(&["cd"]) {
        Commands::Cd { path } => assert_eq!(path, None),
        _ => panic!("expected Cd"),
    }
}

#[test]
fn parses_put_with_optional_args_defaulting_to_none() {
    match parse(&["put", "notes.txt"]) {
        Commands::Put { path, vault, dest } => {
            assert_eq!(path, "notes.txt");
            assert_eq!(vault, None);
            assert_eq!(dest, None);
        }
        _ => panic!("expected Put"),
    }
}

#[test]
fn parses_put_with_all_args() {
    match parse(&["put", "notes.txt", "myvault", "docs"]) {
        Commands::Put { path, vault, dest } => {
            assert_eq!(path, "notes.txt");
            assert_eq!(vault, Some("myvault".to_string()));
            assert_eq!(dest, Some("docs".to_string()));
        }
        _ => panic!("expected Put"),
    }
}

#[test]
fn parses_get() {
    match parse(&["get", "notes.txt", "myvault"]) {
        Commands::Get { path, vault, dest } => {
            assert_eq!(path, "notes.txt");
            assert_eq!(vault, Some("myvault".to_string()));
            assert_eq!(dest, None);
        }
        _ => panic!("expected Get"),
    }
}

#[test]
fn parses_delete_with_force_flag() {
    match parse(&["delete", "notes.txt", "--force"]) {
        Commands::Delete { path, vault, force } => {
            assert_eq!(path, "notes.txt");
            assert_eq!(vault, None);
            assert!(force);
        }
        _ => panic!("expected Delete"),
    }
}

#[test]
fn parses_delete_without_force_flag_defaults_to_false() {
    match parse(&["delete", "notes.txt"]) {
        Commands::Delete { force, .. } => assert!(!force),
        _ => panic!("expected Delete"),
    }
}

#[test]
fn parses_cp_and_mv() {
    match parse(&["cp", "a.txt", "dir"]) {
        Commands::Cp {
            path, destination, ..
        } => {
            assert_eq!(path, "a.txt");
            assert_eq!(destination, "dir");
        }
        _ => panic!("expected Cp"),
    }
    match parse(&["mv", "a.txt", "dir"]) {
        Commands::Mv {
            path, destination, ..
        } => {
            assert_eq!(path, "a.txt");
            assert_eq!(destination, "dir");
        }
        _ => panic!("expected Mv"),
    }
}

#[test]
fn parses_push_and_pull_with_optional_vault() {
    assert!(matches!(parse(&["push"]), Commands::Push { vault: None }));
    match parse(&["pull", "myvault"]) {
        Commands::Pull { vault } => assert_eq!(vault, Some("myvault".to_string())),
        _ => panic!("expected Pull"),
    }
}

#[test]
fn missing_required_argument_fails_to_parse() {
    // `select` requires a vault name; omitting it should fail parsing.
    let result = Cli::try_parse_from(["nimbus", "select"]);
    assert!(result.is_err());
}

#[test]
fn unknown_subcommand_fails_to_parse() {
    let result = Cli::try_parse_from(["nimbus", "not-a-command"]);
    assert!(result.is_err());
}
