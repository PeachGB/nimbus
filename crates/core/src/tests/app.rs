use super::*;
use nimbus_vault::origin::fs::OriginFileSystem;

fn fs_vault(name: &str, root: PathBuf) -> Vault {
    let origin = OriginFileSystem::new(root);
    Vault::from_parts(name.to_string(), Arc::new(origin), ObjectId::from("")).unwrap()
}

fn make_app(vaults: HashMap<String, Vault>) -> App {
    App {
        vaults,
        vault_configs: HashMap::new(),
        cwd: ObjectId::from(APP_ROOT_ID),
        cwd_path: PathBuf::from("/"),
        current_vault: None,
        local_root: None,
        local_root_canonical: None,
    }
}

// --- resolve_relative / normalize ---

#[test]
fn resolve_relative_absolute_input_ignores_current() {
    let result = App::resolve_relative(Path::new("/some/where"), "/other/place");
    assert_eq!(result, PathBuf::from("/other/place"));
}

#[test]
fn resolve_relative_relative_input_appends_to_current() {
    let result = App::resolve_relative(Path::new("/docs"), "notes");
    assert_eq!(result, PathBuf::from("/docs/notes"));
}

#[test]
fn resolve_relative_handles_parent_dir() {
    let result = App::resolve_relative(Path::new("/docs/2024"), "..");
    assert_eq!(result, PathBuf::from("/docs"));
}

#[test]
fn resolve_relative_parent_dir_at_root_stays_at_root() {
    let result = App::resolve_relative(Path::new("/"), "..");
    assert_eq!(result, PathBuf::from("/"));
}

#[test]
fn resolve_relative_handles_current_dir_component() {
    let result = App::resolve_relative(Path::new("/docs"), "./notes");
    assert_eq!(result, PathBuf::from("/docs/notes"));
}

// --- resolve_local_path ---

#[test]
fn resolve_local_path_within_root_returns_relative_vault_path() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("notes.txt"), b"hi").unwrap();
    let mut app = make_app(HashMap::new());
    app.local_root_canonical = Some(root.path().canonicalize().unwrap());

    let result = app
        .resolve_local_path(root.path().join("notes.txt").to_str().unwrap())
        .unwrap();
    assert_eq!(result, PathBuf::from("/notes.txt"));
}

#[test]
fn resolve_local_path_outside_root_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), b"hi").unwrap();
    let mut app = make_app(HashMap::new());
    app.local_root_canonical = Some(root.path().canonicalize().unwrap());

    let result = app.resolve_local_path(outside.path().join("secret.txt").to_str().unwrap());
    assert!(result.is_err());
}

#[test]
fn resolve_local_path_nonexistent_path_errors() {
    let root = tempfile::tempdir().unwrap();
    let mut app = make_app(HashMap::new());
    app.local_root_canonical = Some(root.path().canonicalize().unwrap());

    let result = app.resolve_local_path(root.path().join("missing.txt").to_str().unwrap());
    assert!(result.is_err());
}

#[test]
fn resolve_local_path_errors_when_no_local_vault_configured() {
    let app = make_app(HashMap::new());
    let result = app.resolve_local_path("/tmp");
    assert!(result.is_err());
}

// --- select ---

#[test]
fn select_sets_current_vault_and_resets_cwd() {
    let root = tempfile::tempdir().unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);

    app.select("v1".to_string()).unwrap();
    assert_eq!(app.current_vault.as_deref(), Some("v1"));
    assert_eq!(app.cwd_path, PathBuf::from("/"));
    assert_eq!(app.cwd.as_str(), "");
}

#[test]
fn select_unknown_vault_errors() {
    let mut app = make_app(HashMap::new());
    assert!(app.select("missing".to_string()).is_err());
}

// --- new ---

#[test]
fn new_registers_vault_from_config_file() {
    let data_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let config_path = config_dir.path().join("vault.toml");
    std::fs::write(
        &config_path,
        format!(
            "name = \"my-vault\"\n\n[origin_config]\ntype = \"fs\"\nroot = \"{}\"\n",
            data_dir.path().display()
        ),
    )
    .unwrap();

    let mut app = make_app(HashMap::new());
    app.new_vault(config_path.clone()).unwrap();

    assert!(app.vaults.contains_key("my-vault"));
    assert_eq!(app.vault_configs.get("my-vault"), Some(&config_path));
}

// --- cd ---

#[tokio::test]
async fn cd_at_root_level_selects_vault_and_recurses_into_remaining_path() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("docs")).unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);

    app.cd(Some("v1/docs".to_string())).await.unwrap();

    assert_eq!(app.current_vault.as_deref(), Some("v1"));
    assert_eq!(app.cwd_path, PathBuf::from("/docs"));
}

#[tokio::test]
async fn cd_within_vault_resolves_relative_to_cwd() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("docs")).unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();
    app.cd(Some("docs".to_string())).await.unwrap();
    assert_eq!(app.cwd_path, PathBuf::from("/docs"));

    app.cd(Some("..".to_string())).await.unwrap();
    assert_eq!(app.cwd_path, PathBuf::from("/"));
}

#[tokio::test]
async fn cd_unknown_path_component_errors() {
    let root = tempfile::tempdir().unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    let result = app.cd(Some("missing".to_string())).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn cd_at_root_level_with_unknown_vault_errors() {
    let mut app = make_app(HashMap::new());
    let result = app.cd(Some("missing".to_string())).await;
    assert!(result.is_err());
}

// --- ls ---

#[tokio::test]
async fn ls_with_no_current_vault_delegates_to_vaults() {
    let app = make_app(HashMap::new());
    assert!(app.ls().await.is_ok());
}

#[tokio::test]
async fn ls_lists_current_vault_contents() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"hi").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    assert!(app.ls().await.is_ok());
}

// --- put / get ---

#[tokio::test]
async fn put_copies_local_file_into_target_vault() {
    let local_root = tempfile::tempdir().unwrap();
    std::fs::write(local_root.path().join("note.txt"), b"hello").unwrap();
    let target_root = tempfile::tempdir().unwrap();

    let mut vaults = HashMap::new();
    vaults.insert(
        LOCAL_VAULT_NAME.to_string(),
        fs_vault(LOCAL_VAULT_NAME, local_root.path().to_path_buf()),
    );
    vaults.insert(
        "v1".to_string(),
        fs_vault("v1", target_root.path().to_path_buf()),
    );
    let mut app = make_app(vaults);
    app.local_root_canonical = Some(local_root.path().canonicalize().unwrap());

    let source = local_root.path().join("note.txt");
    app.put(
        source.to_str().unwrap().to_string(),
        Some("v1".to_string()),
        None,
    )
    .await
    .unwrap();

    let contents = std::fs::read(target_root.path().join("note.txt")).unwrap();
    assert_eq!(contents, b"hello");
}

#[tokio::test]
async fn put_without_local_vault_errors() {
    let target_root = tempfile::tempdir().unwrap();
    let mut vaults = HashMap::new();
    vaults.insert(
        "v1".to_string(),
        fs_vault("v1", target_root.path().to_path_buf()),
    );
    let mut app = make_app(vaults);

    let result = app
        .put("/nope".to_string(), Some("v1".to_string()), None)
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn get_copies_vault_file_into_local_root() {
    let source_root = tempfile::tempdir().unwrap();
    std::fs::write(source_root.path().join("note.txt"), b"remote-data").unwrap();
    let local_root = tempfile::tempdir().unwrap();

    let mut vaults = HashMap::new();
    vaults.insert(
        "v1".to_string(),
        fs_vault("v1", source_root.path().to_path_buf()),
    );
    vaults.insert(
        LOCAL_VAULT_NAME.to_string(),
        fs_vault(LOCAL_VAULT_NAME, local_root.path().to_path_buf()),
    );
    let mut app = make_app(vaults);
    app.local_root_canonical = Some(local_root.path().canonicalize().unwrap());

    app.get(
        "note.txt".to_string(),
        Some("v1".to_string()),
        Some(local_root.path().to_str().unwrap().to_string()),
    )
    .await
    .unwrap();

    let contents = std::fs::read(local_root.path().join("note.txt")).unwrap();
    assert_eq!(contents, b"remote-data");
}

// --- cp / mv ---

#[tokio::test]
async fn cp_duplicates_object_within_same_vault() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"data").unwrap();
    std::fs::create_dir_all(root.path().join("dir")).unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    app.cp(
        "a.txt".to_string(),
        "dir".to_string(),
        Some("v1".to_string()),
    )
    .await
    .unwrap();

    assert!(root.path().join("a.txt").exists());
    assert_eq!(
        std::fs::read(root.path().join("dir").join("a.txt")).unwrap(),
        b"data"
    );
}

#[tokio::test]
async fn mv_moves_object_and_deletes_source() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"data").unwrap();
    std::fs::create_dir_all(root.path().join("dir")).unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    app.mv(
        "a.txt".to_string(),
        "dir".to_string(),
        Some("v1".to_string()),
    )
    .await
    .unwrap();

    assert!(!root.path().join("a.txt").exists());
    assert_eq!(
        std::fs::read(root.path().join("dir").join("a.txt")).unwrap(),
        b"data"
    );
}

// --- delete ---

#[tokio::test]
async fn delete_removes_empty_directory_without_force() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("dir")).unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    app.delete("dir".to_string(), Some("v1".to_string()), false)
        .await
        .unwrap();
    assert!(!root.path().join("dir").exists());
}

#[tokio::test]
async fn delete_non_empty_directory_without_force_errors() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("dir")).unwrap();
    std::fs::write(root.path().join("dir").join("a.txt"), b"data").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    let result = app
        .delete("dir".to_string(), Some("v1".to_string()), false)
        .await;
    assert!(result.is_err());
    assert!(root.path().join("dir").exists());
}

#[tokio::test]
async fn delete_non_empty_directory_with_force_succeeds() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("dir")).unwrap();
    std::fs::write(root.path().join("dir").join("a.txt"), b"data").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    app.delete("dir".to_string(), Some("v1".to_string()), true)
        .await
        .unwrap();
    assert!(!root.path().join("dir").exists());
}

// --- push / pull ---

#[tokio::test]
async fn push_syncs_local_vault_contents_into_target_vault() {
    let local_root = tempfile::tempdir().unwrap();
    std::fs::write(local_root.path().join("a.txt"), b"local-data").unwrap();
    let target_root = tempfile::tempdir().unwrap();

    let mut vaults = HashMap::new();
    vaults.insert(
        LOCAL_VAULT_NAME.to_string(),
        fs_vault(LOCAL_VAULT_NAME, local_root.path().to_path_buf()),
    );
    vaults.insert(
        "v1".to_string(),
        fs_vault("v1", target_root.path().to_path_buf()),
    );
    let mut app = make_app(vaults);

    app.push(Some("v1".to_string())).await.unwrap();

    assert_eq!(
        std::fs::read(target_root.path().join("a.txt")).unwrap(),
        b"local-data"
    );
}

#[tokio::test]
async fn pull_syncs_source_vault_contents_into_local_vault() {
    let source_root = tempfile::tempdir().unwrap();
    std::fs::write(source_root.path().join("a.txt"), b"remote-data").unwrap();
    let local_root = tempfile::tempdir().unwrap();

    let mut vaults = HashMap::new();
    vaults.insert(
        "v1".to_string(),
        fs_vault("v1", source_root.path().to_path_buf()),
    );
    vaults.insert(
        LOCAL_VAULT_NAME.to_string(),
        fs_vault(LOCAL_VAULT_NAME, local_root.path().to_path_buf()),
    );
    let mut app = make_app(vaults);

    app.pull(Some("v1".to_string())).await.unwrap();

    assert_eq!(
        std::fs::read(local_root.path().join("a.txt")).unwrap(),
        b"remote-data"
    );
}

// --- cd_completions ---

#[tokio::test]
async fn cd_completions_with_no_vault_selected_lists_matching_vault_names() {
    let root1 = tempfile::tempdir().unwrap();
    let root2 = tempfile::tempdir().unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root1.path().to_path_buf()));
    vaults.insert("v2".to_string(), fs_vault("v2", root2.path().to_path_buf()));
    let app = make_app(vaults);

    let mut candidates = app.cd_completions("v").await;
    candidates.sort();
    assert_eq!(candidates, vec!["v1".to_string(), "v2".to_string()]);

    assert!(app.cd_completions("zz").await.is_empty());
}

#[tokio::test]
async fn cd_completions_lists_subdirectories_of_current_directory() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("docs")).unwrap();
    std::fs::create_dir(root.path().join("downloads")).unwrap();
    std::fs::write(root.path().join("notes.txt"), b"hi").unwrap();

    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    let mut candidates = app.cd_completions("do").await;
    candidates.sort();
    assert_eq!(candidates, vec!["docs".to_string(), "downloads".to_string()]);

    // Files aren't valid `cd` targets, so they shouldn't be suggested.
    assert!(app.cd_completions("notes").await.is_empty());
}

#[tokio::test]
async fn cd_completions_descends_into_nested_path_segments() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("docs/2024")).unwrap();
    std::fs::create_dir_all(root.path().join("docs/2025")).unwrap();

    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    let mut candidates = app.cd_completions("docs/20").await;
    candidates.sort();
    assert_eq!(
        candidates,
        vec!["docs/2024".to_string(), "docs/2025".to_string()]
    );
}

#[tokio::test]
async fn cd_completions_with_no_vault_selected_and_slash_lists_vault_directories() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("docs")).unwrap();

    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let app = make_app(vaults);

    let candidates = app.cd_completions("v1/do").await;
    assert_eq!(candidates, vec!["v1/docs".to_string()]);
}
