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
        // Never the real session file: anything calling `save()` (new_vault, forget_vault)
        // would otherwise rewrite the developer's own vault registry from a test run.
        state_path: scratch_state_path(),
        saved_session: None,
        registered: HashSet::new(),
        forgotten: HashSet::new(),
    }
}

/// A unique throwaway path for a test app's `save()` target.
fn scratch_state_path() -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir()
        .join("nimbus-test-state")
        .join(format!("{}-{n}.toml", std::process::id()))
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

/// Writes a vault config naming `name` and returns its path.
fn write_vault_config(dir: &std::path::Path, file: &str, name: &str) -> PathBuf {
    let data_dir = dir.join(format!("{file}-data"));
    std::fs::create_dir_all(&data_dir).unwrap();
    let path = dir.join(file);
    std::fs::write(
        &path,
        format!(
            "name = \"{name}\"\n\n[origin_config]\ntype = \"fs\"\nroot = \"{}\"\n",
            data_dir.display()
        ),
    )
    .unwrap();
    path
}

#[test]
fn new_vault_refuses_a_name_already_taken_by_a_different_config() {
    let dir = tempfile::tempdir().unwrap();
    let first = write_vault_config(dir.path(), "first.toml", "shared-name");
    let second = write_vault_config(dir.path(), "second.toml", "shared-name");

    let mut app = make_app(HashMap::new());
    app.new_vault(first.clone()).unwrap();

    // Silently replacing would leave the first vault unreachable with nothing pointing at it.
    assert!(app.new_vault(second).is_err());
    assert_eq!(app.vault_configs.get("shared-name"), Some(&first));
}

#[test]
fn new_vault_re_registering_the_same_config_is_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_vault_config(dir.path(), "v.toml", "reloadable");

    let mut app = make_app(HashMap::new());
    app.new_vault(path.clone()).unwrap();
    // Re-registering is how an edit to the config file gets picked up.
    app.new_vault(path.clone()).unwrap();
    assert_eq!(app.vault_configs.get("reloadable"), Some(&path));
}

/// `path` (absolute) spelled relative to the current directory, the way a session written
/// before registration canonicalized paths could hold it, without changing the cwd.
fn relative_from_cwd(path: &Path) -> PathBuf {
    let cwd = std::env::current_dir().unwrap();
    let mut relative: PathBuf = std::iter::repeat_n("..", cwd.components().count() - 1).collect();
    relative.push(path.strip_prefix("/").unwrap());
    relative
}

#[test]
fn new_vault_updates_an_old_relative_entry_for_the_same_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_vault_config(dir.path(), "v.toml", "upgraded");
    let mut app = make_app(HashMap::new());
    app.vault_configs
        .insert("upgraded".to_string(), relative_from_cwd(&path));

    app.new_vault(path.clone()).unwrap();

    assert_eq!(
        app.vault_configs.get("upgraded"),
        Some(&path.canonicalize().unwrap())
    );
}

#[test]
fn new_vault_stores_the_canonical_absolute_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_vault_config(dir.path(), "v.toml", "canonical");
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    // Relative to the cwd and with a `..` in it: two ways a typed path differs from the file.
    let typed = relative_from_cwd(&dir.path().join("sub/../v.toml"));

    let mut app = make_app(HashMap::new());
    app.new_vault(typed).unwrap();

    let stored = app.vault_configs.get("canonical").unwrap();
    assert!(stored.is_absolute());
    assert_eq!(stored, &path.canonicalize().unwrap());
}

#[test]
fn new_vault_names_a_config_that_does_not_exist() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.toml");
    let mut app = make_app(HashMap::new());
    let error = app.new_vault(missing.clone()).unwrap_err().to_string();
    assert!(error.contains(&missing.display().to_string()), "{error}");
}

#[test]
fn forget_vault_frees_the_name_for_a_different_config() {
    let dir = tempfile::tempdir().unwrap();
    let first = write_vault_config(dir.path(), "first.toml", "shared-name");
    let second = write_vault_config(dir.path(), "second.toml", "shared-name");

    let mut app = make_app(HashMap::new());
    app.new_vault(first).unwrap();
    app.forget_vault("shared-name".to_string()).unwrap();

    app.new_vault(second.clone()).unwrap();
    assert_eq!(app.vault_configs.get("shared-name"), Some(&second));
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

// --- session persistence ---

#[tokio::test]
async fn restore_session_returns_to_the_saved_vault_and_directory() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("docs/2024")).unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.saved_session = Some(("v1".to_string(), PathBuf::from("/docs/2024")));

    app.restore_session().await.unwrap();

    assert_eq!(app.current_vault.as_deref(), Some("v1"));
    assert_eq!(app.cwd_path, PathBuf::from("/docs/2024"));
}

#[tokio::test]
async fn restore_session_falls_back_to_the_vault_root_when_the_directory_is_gone() {
    let root = tempfile::tempdir().unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.saved_session = Some(("v1".to_string(), PathBuf::from("/gone")));

    app.restore_session().await.unwrap();

    assert_eq!(app.current_vault.as_deref(), Some("v1"));
    assert_eq!(app.cwd_path, PathBuf::from("/"));
}

#[tokio::test]
async fn restore_session_falls_back_to_the_app_root_when_the_vault_is_gone() {
    let mut app = make_app(HashMap::new());
    app.saved_session = Some(("forgotten".to_string(), PathBuf::from("/docs")));

    app.restore_session().await.unwrap();

    assert_eq!(app.current_vault, None);
    assert_eq!(app.cwd_path, PathBuf::from("/"));
}

/// A vault that's registered but didn't open this run (its `token_env` isn't exported, say) is
/// a *temporary* failure, so the saved position has to survive it — otherwise the next `save()`
/// resets you to the root and where you were is gone for good.
#[tokio::test]
async fn restore_session_keeps_the_position_of_a_registered_vault_that_didnt_open() {
    let mut app = make_app(HashMap::new());
    app.vault_configs
        .insert("v1".to_string(), PathBuf::from("/does/not/build.toml"));
    app.saved_session = Some(("v1".to_string(), PathBuf::from("/docs")));

    app.restore_session().await.unwrap();

    // Not restored — the vault isn't open — but not forgotten either.
    assert_eq!(app.current_vault, None);
    assert_eq!(
        app.saved_session,
        Some(("v1".to_string(), PathBuf::from("/docs")))
    );

    app.save().unwrap();
    let raw = std::fs::read_to_string(&app.state_path).unwrap();
    let saved: SavedState = toml::from_str(&raw).unwrap();
    assert_eq!(saved.current_vault.as_deref(), Some("v1"));
    assert_eq!(saved.cwd_path, Some(PathBuf::from("/docs")));
}

/// The other half of the rule above: a vault that isn't in the registry at all is really gone,
/// so its position goes too rather than warning on every run forever.
#[tokio::test]
async fn restore_session_drops_the_position_of_an_unregistered_vault() {
    let mut app = make_app(HashMap::new());
    app.saved_session = Some(("forgotten".to_string(), PathBuf::from("/docs")));

    app.restore_session().await.unwrap();

    assert_eq!(app.saved_session, None);

    app.save().unwrap();
    let raw = std::fs::read_to_string(&app.state_path).unwrap();
    let saved: SavedState = toml::from_str(&raw).unwrap();
    assert_eq!(saved.current_vault, None);
}

#[tokio::test]
async fn save_then_init_round_trips_the_current_vault_and_directory() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("docs")).unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();
    app.cd(Some("docs".to_string())).await.unwrap();
    app.save().unwrap();

    let raw = std::fs::read_to_string(&app.state_path).unwrap();
    let saved: SavedState = toml::from_str(&raw).unwrap();
    assert_eq!(saved.current_vault.as_deref(), Some("v1"));
    assert_eq!(saved.cwd_path, Some(PathBuf::from("/docs")));
}

#[test]
fn save_keeps_a_session_that_was_never_restored() {
    let mut app = make_app(HashMap::new());
    app.saved_session = Some(("v1".to_string(), PathBuf::from("/docs")));

    app.save().unwrap();

    let raw = std::fs::read_to_string(&app.state_path).unwrap();
    let saved: SavedState = toml::from_str(&raw).unwrap();
    assert_eq!(saved.current_vault.as_deref(), Some("v1"));
    assert_eq!(saved.cwd_path, Some(PathBuf::from("/docs")));
}

#[test]
fn a_state_file_without_session_keys_still_loads() {
    let saved: SavedState = toml::from_str("[vault_configs]\n").unwrap();
    assert_eq!(saved.current_vault, None);
    assert_eq!(saved.cwd_path, None);
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

#[tokio::test]
async fn get_without_a_destination_lands_in_the_local_root() {
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

    // No destination: this must not depend on the process's working directory, which is
    // almost never inside the local root.
    app.get("note.txt".to_string(), Some("v1".to_string()), None)
        .await
        .unwrap();

    assert_eq!(
        std::fs::read(local_root.path().join("note.txt")).unwrap(),
        b"remote-data"
    );
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

#[tokio::test]
async fn cp_recursively_copies_directory_contents_within_same_vault() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("src/nested")).unwrap();
    std::fs::write(root.path().join("src/a.txt"), b"top-level").unwrap();
    std::fs::write(root.path().join("src/nested/b.txt"), b"nested").unwrap();
    std::fs::create_dir_all(root.path().join("dst")).unwrap();

    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    app.cp("src".to_string(), "dst".to_string(), Some("v1".to_string()))
        .await
        .unwrap();

    // source subtree is untouched
    assert!(root.path().join("src/a.txt").exists());
    assert!(root.path().join("src/nested/b.txt").exists());
    // destination got a full recursive copy, not just an empty stub directory
    assert_eq!(
        std::fs::read(root.path().join("dst/src/a.txt")).unwrap(),
        b"top-level"
    );
    assert_eq!(
        std::fs::read(root.path().join("dst/src/nested/b.txt")).unwrap(),
        b"nested"
    );
}

#[tokio::test]
async fn cp_copies_object_between_two_vaults_with_the_same_origin_type() {
    let source_root = tempfile::tempdir().unwrap();
    std::fs::write(source_root.path().join("a.txt"), b"data").unwrap();
    let dest_root = tempfile::tempdir().unwrap();

    let mut vaults = HashMap::new();
    vaults.insert(
        "source".to_string(),
        fs_vault("source", source_root.path().to_path_buf()),
    );
    vaults.insert(
        "dest".to_string(),
        fs_vault("dest", dest_root.path().to_path_buf()),
    );
    let mut app = make_app(vaults);

    app.cp(
        "a.txt".to_string(),
        "dest:/".to_string(),
        Some("source".to_string()),
    )
    .await
    .unwrap();

    // source is untouched, destination's own origin (a different directory) got the file
    assert!(source_root.path().join("a.txt").exists());
    assert_eq!(
        std::fs::read(dest_root.path().join("a.txt")).unwrap(),
        b"data"
    );
}

#[tokio::test]
async fn mv_moves_directory_between_two_vaults_with_the_same_origin_type() {
    let source_root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(source_root.path().join("dir/nested")).unwrap();
    std::fs::write(source_root.path().join("dir/a.txt"), b"top-level").unwrap();
    std::fs::write(source_root.path().join("dir/nested/b.txt"), b"nested").unwrap();
    let dest_root = tempfile::tempdir().unwrap();

    let mut vaults = HashMap::new();
    vaults.insert(
        "source".to_string(),
        fs_vault("source", source_root.path().to_path_buf()),
    );
    vaults.insert(
        "dest".to_string(),
        fs_vault("dest", dest_root.path().to_path_buf()),
    );
    let mut app = make_app(vaults);

    app.mv(
        "dir".to_string(),
        "dest:/".to_string(),
        Some("source".to_string()),
    )
    .await
    .unwrap();

    // source's whole subtree is gone (recursive delete), destination has the full tree
    assert!(!source_root.path().join("dir").exists());
    assert_eq!(
        std::fs::read(dest_root.path().join("dir/a.txt")).unwrap(),
        b"top-level"
    );
    assert_eq!(
        std::fs::read(dest_root.path().join("dir/nested/b.txt")).unwrap(),
        b"nested"
    );
}

#[tokio::test]
async fn cp_copies_directory_between_vaults_with_different_origin_types() {
    use nimbus_vault::origin::vault::OriginVault;

    // "source": a plain fs-backed vault.
    let source_root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(source_root.path().join("dir/nested")).unwrap();
    std::fs::write(source_root.path().join("dir/a.txt"), b"top-level").unwrap();
    std::fs::write(source_root.path().join("dir/nested/b.txt"), b"nested").unwrap();

    // "dest": backed by `OriginVault`, wrapping a *second* fs-backed vault — a genuinely
    // different `Origin` implementation than `OriginFileSystem`, even though it bottoms out
    // on disk too.
    let inner_root = tempfile::tempdir().unwrap();
    let inner_vault = Arc::new(fs_vault("inner", inner_root.path().to_path_buf()));
    let dest_origin = OriginVault::new(inner_vault);
    let dest_vault = Vault::from_parts(
        "dest".to_string(),
        Arc::new(dest_origin),
        ObjectId::from(""),
    )
    .unwrap();

    let mut vaults = HashMap::new();
    vaults.insert(
        "source".to_string(),
        fs_vault("source", source_root.path().to_path_buf()),
    );
    vaults.insert("dest".to_string(), dest_vault);
    let mut app = make_app(vaults);

    app.cp(
        "dir".to_string(),
        "dest:/".to_string(),
        Some("source".to_string()),
    )
    .await
    .unwrap();

    // the copy landed on the *inner* fs vault's real directory, proving it went through
    // `OriginVault` -> the wrapped vault -> its own `OriginFileSystem`, not some in-memory shim
    assert_eq!(
        std::fs::read(inner_root.path().join("dir/a.txt")).unwrap(),
        b"top-level"
    );
    assert_eq!(
        std::fs::read(inner_root.path().join("dir/nested/b.txt")).unwrap(),
        b"nested"
    );
}

// --- vault:path spec parsing ---

#[test]
fn split_vault_spec_recognises_a_known_vault_prefix() {
    let root = tempfile::tempdir().unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let app = make_app(vaults);

    assert_eq!(
        app.split_vault_spec("v1:/docs"),
        (Some("v1".to_string()), "/docs".to_string())
    );
    assert_eq!(
        app.split_vault_spec("v1:"),
        (Some("v1".to_string()), String::new())
    );
}

#[test]
fn split_vault_spec_leaves_unqualified_and_unknown_prefixes_as_paths() {
    let root = tempfile::tempdir().unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let app = make_app(vaults);

    // a plain relative path
    assert_eq!(
        app.split_vault_spec("docs/notes.txt"),
        (None, "docs/notes.txt".to_string())
    );
    // a colon that isn't a known vault stays part of the object's name
    assert_eq!(
        app.split_vault_spec("weird:name.txt"),
        (None, "weird:name.txt".to_string())
    );
}

#[test]
fn resolve_spec_qualified_resolves_from_that_vaults_root_not_the_current_cwd() {
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    vaults.insert("v2".to_string(), fs_vault("v2", other.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();
    app.cwd_path = PathBuf::from("/deep/nested");

    // qualified: ignores v1's cwd entirely, resolves from v2's root
    let (vault, path) = app.resolve_spec("v2:/inbox", None).unwrap();
    assert_eq!(vault, "v2");
    assert_eq!(path, PathBuf::from("/inbox"));

    // unqualified: still relative to the current vault's cwd
    let (vault, path) = app.resolve_spec("sub", None).unwrap();
    assert_eq!(vault, "v1");
    assert_eq!(path, PathBuf::from("/deep/nested/sub"));
}

#[tokio::test]
async fn cp_a_directory_into_its_own_subtree_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("dir/sub")).unwrap();
    std::fs::write(root.path().join("dir/a.txt"), b"data").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    let result = app.cp("dir".to_string(), "dir/sub".to_string(), None).await;
    assert!(result.is_err());
    // the source survived rather than being recursed into forever / half-copied
    assert_eq!(
        std::fs::read(root.path().join("dir/a.txt")).unwrap(),
        b"data"
    );
}

#[tokio::test]
async fn cp_an_object_onto_its_own_parent_directory_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"data").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    // `put` truncates before `send` writes, so without the guard this would empty a.txt
    let result = app.cp("a.txt".to_string(), ".".to_string(), None).await;
    assert!(result.is_err());
    assert_eq!(std::fs::read(root.path().join("a.txt")).unwrap(), b"data");
}

// --- write_object_bytes ---

#[tokio::test]
async fn write_object_bytes_round_trips_through_the_origin() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"original").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    let id = app.list_cwd().await.unwrap()[0].get_id();
    assert_eq!(
        app.fetch_object_bytes(id.clone()).await.unwrap(),
        b"original"
    );

    app.write_object_bytes(id.clone(), b"edited elsewhere".to_vec())
        .await
        .unwrap();

    // landed on the real origin, not just the in-memory cache
    assert_eq!(
        std::fs::read(root.path().join("a.txt")).unwrap(),
        b"edited elsewhere"
    );
    assert_eq!(
        app.fetch_object_bytes(id).await.unwrap(),
        b"edited elsewhere"
    );
}

// --- mkdir ---

#[tokio::test]
async fn mkdir_creates_a_directory_relative_to_the_cwd() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("parent")).unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();
    app.cd(Some("parent".to_string())).await.unwrap();

    app.mkdir("child".to_string(), None).await.unwrap();
    assert!(root.path().join("parent/child").is_dir());
}

#[tokio::test]
async fn mkdir_into_another_vault_via_a_qualified_spec() {
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    vaults.insert("v2".to_string(), fs_vault("v2", other.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    app.mkdir("v2:/inbox".to_string(), None).await.unwrap();
    assert!(other.path().join("inbox").is_dir());
    assert!(!root.path().join("inbox").exists());
}

#[tokio::test]
async fn mkdir_refuses_to_clobber_an_existing_entry() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("taken"), b"data").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    assert!(app.mkdir("taken".to_string(), None).await.is_err());
    // the existing file is untouched, not replaced by a directory
    assert_eq!(std::fs::read(root.path().join("taken")).unwrap(), b"data");
}

// --- rename ---

#[tokio::test]
async fn rename_renames_a_file_in_place() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("before.txt"), b"data").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    app.rename("before.txt".to_string(), "after.txt".to_string(), None)
        .await
        .unwrap();

    assert!(!root.path().join("before.txt").exists());
    assert_eq!(
        std::fs::read(root.path().join("after.txt")).unwrap(),
        b"data"
    );
}

#[tokio::test]
async fn rename_a_directory_keeps_its_contents_and_their_names() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("before/sub")).unwrap();
    std::fs::write(root.path().join("before/top.txt"), b"top").unwrap();
    std::fs::write(root.path().join("before/sub/nested.txt"), b"nested").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    app.rename("before".to_string(), "after".to_string(), None)
        .await
        .unwrap();

    assert!(!root.path().join("before").exists());
    // only the top level is renamed — descendants keep their own names
    assert_eq!(
        std::fs::read(root.path().join("after/top.txt")).unwrap(),
        b"top"
    );
    assert_eq!(
        std::fs::read(root.path().join("after/sub/nested.txt")).unwrap(),
        b"nested"
    );
}

#[tokio::test]
async fn rename_stays_in_the_objects_own_directory() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("dir")).unwrap();
    std::fs::write(root.path().join("dir/a.txt"), b"data").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();
    app.cd(Some("dir".to_string())).await.unwrap();

    app.rename("a.txt".to_string(), "b.txt".to_string(), None)
        .await
        .unwrap();

    assert_eq!(
        std::fs::read(root.path().join("dir/b.txt")).unwrap(),
        b"data"
    );
    assert!(!root.path().join("b.txt").exists());
}

#[tokio::test]
async fn rename_refuses_to_overwrite_an_existing_name() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"a-data").unwrap();
    std::fs::write(root.path().join("b.txt"), b"b-data").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    assert!(
        app.rename("a.txt".to_string(), "b.txt".to_string(), None)
            .await
            .is_err()
    );
    // both survive intact — no truncation of the victim, no loss of the source
    assert_eq!(std::fs::read(root.path().join("a.txt")).unwrap(), b"a-data");
    assert_eq!(std::fs::read(root.path().join("b.txt")).unwrap(), b"b-data");
}

#[tokio::test]
async fn rename_rejects_names_that_would_move_the_object() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"data").unwrap();
    std::fs::create_dir_all(root.path().join("dir")).unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    // a rename takes a name, not a path — use `mv` to relocate
    assert!(
        app.rename("a.txt".to_string(), "dir/a.txt".to_string(), None)
            .await
            .is_err()
    );
    assert!(
        app.rename("a.txt".to_string(), String::new(), None)
            .await
            .is_err()
    );
    assert!(root.path().join("a.txt").exists());
}

// --- cp/mv to a new name ---

#[tokio::test]
async fn cp_to_a_nonexistent_path_copies_under_that_new_name() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"data").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    app.cp("a.txt".to_string(), "copy.txt".to_string(), None)
        .await
        .unwrap();

    assert_eq!(std::fs::read(root.path().join("a.txt")).unwrap(), b"data");
    assert_eq!(
        std::fs::read(root.path().join("copy.txt")).unwrap(),
        b"data"
    );
}

#[tokio::test]
async fn cp_to_a_new_name_in_another_directory() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("docs")).unwrap();
    std::fs::write(root.path().join("a.txt"), b"data").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    app.cp("a.txt".to_string(), "docs/renamed.txt".to_string(), None)
        .await
        .unwrap();

    assert_eq!(
        std::fs::read(root.path().join("docs/renamed.txt")).unwrap(),
        b"data"
    );
}

#[tokio::test]
async fn cp_to_a_new_name_across_vaults() {
    let root1 = tempfile::tempdir().unwrap();
    let root2 = tempfile::tempdir().unwrap();
    std::fs::write(root1.path().join("a.txt"), b"data").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root1.path().to_path_buf()));
    vaults.insert("v2".to_string(), fs_vault("v2", root2.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    app.cp("a.txt".to_string(), "v2:/landed.txt".to_string(), None)
        .await
        .unwrap();

    assert_eq!(
        std::fs::read(root2.path().join("landed.txt")).unwrap(),
        b"data"
    );
}

#[tokio::test]
async fn cp_a_directory_to_a_new_name_keeps_its_contents() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("docs/nested")).unwrap();
    std::fs::write(root.path().join("docs/a.txt"), b"one").unwrap();
    std::fs::write(root.path().join("docs/nested/b.txt"), b"two").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    app.cp("docs".to_string(), "archive".to_string(), None)
        .await
        .unwrap();

    // only the top level is renamed; descendants keep their own names
    assert_eq!(
        std::fs::read(root.path().join("archive/a.txt")).unwrap(),
        b"one"
    );
    assert_eq!(
        std::fs::read(root.path().join("archive/nested/b.txt")).unwrap(),
        b"two"
    );
    assert!(root.path().join("docs/a.txt").exists());
}

#[tokio::test]
async fn mv_to_a_new_name_renames_and_removes_the_source() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"data").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    app.mv("a.txt".to_string(), "b.txt".to_string(), None)
        .await
        .unwrap();

    assert!(!root.path().join("a.txt").exists());
    assert_eq!(std::fs::read(root.path().join("b.txt")).unwrap(), b"data");
}

#[tokio::test]
async fn cp_onto_an_existing_file_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"source").unwrap();
    std::fs::write(root.path().join("b.txt"), b"precious").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    // `put` truncates, so overwriting would destroy b.txt before anything was copied.
    assert!(
        app.cp("a.txt".to_string(), "b.txt".to_string(), None)
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(root.path().join("b.txt")).unwrap(),
        b"precious"
    );
}

#[tokio::test]
async fn cp_to_a_new_name_under_a_missing_parent_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"data").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    assert!(
        app.cp("a.txt".to_string(), "nope/b.txt".to_string(), None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn cp_a_directory_onto_itself_is_rejected() {
    // The TUI reaches this by yanking a directory, descending into it, and pasting: source and
    // destination resolve to the same path, and `deep_copy` would recurse into the copy it just
    // made until the filesystem gave out.
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("inbox")).unwrap();
    std::fs::write(root.path().join("inbox/a.txt"), b"data").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    assert!(
        app.cp("/inbox".to_string(), "/inbox".to_string(), None)
            .await
            .is_err()
    );
    assert!(!root.path().join("inbox/inbox").exists());
}

#[tokio::test]
async fn cp_to_a_sibling_name_sharing_a_prefix_is_allowed() {
    // `starts_with` is component-wise, so a name that merely shares a textual prefix with the
    // source must not trip the into-itself guard.
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"data").unwrap();
    std::fs::create_dir_all(root.path().join("docs")).unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    app.cp("a.txt".to_string(), "a.txt.bak".to_string(), None)
        .await
        .unwrap();
    app.cp("docs".to_string(), "docs2".to_string(), None)
        .await
        .unwrap();

    assert!(root.path().join("a.txt.bak").is_file());
    assert!(root.path().join("docs2").is_dir());
}

// --- touch ---

#[tokio::test]
async fn touch_creates_an_empty_file() {
    let root = tempfile::tempdir().unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    app.touch("new.txt".to_string(), None).await.unwrap();
    let created = root.path().join("new.txt");
    assert!(created.is_file());
    assert!(std::fs::read(&created).unwrap().is_empty());
}

#[tokio::test]
async fn touch_refuses_to_truncate_an_existing_file() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"precious").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    assert!(app.touch("a.txt".to_string(), None).await.is_err());
    assert_eq!(
        std::fs::read(root.path().join("a.txt")).unwrap(),
        b"precious"
    );
}

#[tokio::test]
async fn touch_creates_into_another_vault_via_a_vault_prefix() {
    let root1 = tempfile::tempdir().unwrap();
    let root2 = tempfile::tempdir().unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root1.path().to_path_buf()));
    vaults.insert("v2".to_string(), fs_vault("v2", root2.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    app.touch("v2:/made.txt".to_string(), None).await.unwrap();
    assert!(root2.path().join("made.txt").is_file());
    assert!(!root1.path().join("made.txt").exists());
}

// --- forget_vault ---

#[test]
fn forget_vault_drops_it_from_the_registry_without_touching_its_data() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"data").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);

    app.forget_vault("v1".to_string()).unwrap();
    assert!(!app.vaults.contains_key("v1"));
    assert!(root.path().join("a.txt").exists());
}

#[test]
fn forget_vault_clears_the_cwd_when_it_was_the_current_vault() {
    let root = tempfile::tempdir().unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    app.forget_vault("v1".to_string()).unwrap();
    assert_eq!(app.current_vault, None);
    assert_eq!(app.cwd_path, PathBuf::from("/"));
}

#[test]
fn forget_vault_refuses_the_local_vault_and_unknown_names() {
    let root = tempfile::tempdir().unwrap();
    let mut vaults = HashMap::new();
    vaults.insert(
        LOCAL_VAULT_NAME.to_string(),
        fs_vault(LOCAL_VAULT_NAME, root.path().to_path_buf()),
    );
    let mut app = make_app(vaults);

    assert!(app.forget_vault(LOCAL_VAULT_NAME.to_string()).is_err());
    assert!(app.vaults.contains_key(LOCAL_VAULT_NAME));
    assert!(app.forget_vault("nope".to_string()).is_err());
}

#[test]
fn forget_vault_works_on_one_that_is_registered_but_could_not_be_opened() {
    // How you get rid of a vault whose config no longer builds — it never made it into
    // `vaults`, only into the registry.
    let mut app = make_app(HashMap::new());
    app.vault_configs
        .insert("broken".to_string(), PathBuf::from("/nonexistent/v.toml"));

    app.forget_vault("broken".to_string()).unwrap();

    assert!(!app.vault_configs.contains_key("broken"));
}

// --- opening the registry ---

#[test]
fn a_vault_whose_config_does_not_build_is_skipped_but_stays_registered() {
    // Its config may only be unbuildable *right now* — an `[origin_config.auth] token_env`
    // that isn't exported in this shell. Dropping the registry entry would have the next
    // `save` unregister the vault for good over that.
    let dir = tempfile::tempdir().unwrap();
    let good_config = dir.path().join("good.toml");
    std::fs::write(
        &good_config,
        format!(
            "name = \"good\"\n\n[origin_config]\ntype = \"fs\"\nroot = \"{}\"\n",
            dir.path().display()
        ),
    )
    .unwrap();

    let registry = HashMap::from([
        ("good".to_string(), good_config),
        ("broken".to_string(), dir.path().join("missing.toml")),
    ]);

    let vaults = App::open_vaults(&registry);

    assert!(vaults.contains_key("good"));
    assert!(!vaults.contains_key("broken"));
    // The registry is borrowed, so what `save` writes still has both.
    assert!(registry.contains_key("broken"));
}

#[test]
fn save_keeps_a_registered_vault_that_could_not_be_opened() {
    let mut app = make_app(HashMap::new());
    app.vault_configs
        .insert("broken".to_string(), PathBuf::from("/nonexistent/v.toml"));

    app.save().unwrap();

    let written: SavedState =
        toml::from_str(&std::fs::read_to_string(&app.state_path).unwrap()).unwrap();
    assert!(written.vault_configs.contains_key("broken"));
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

#[tokio::test]
async fn delete_removes_a_file_without_force() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"data").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    // A leaf has no children to be "not empty" — the emptiness guard must not apply to it.
    app.delete("a.txt".to_string(), None, false).await.unwrap();
    assert!(!root.path().join("a.txt").exists());
}

#[tokio::test]
async fn delete_resolves_a_file_relative_to_the_cwd() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("dir")).unwrap();
    std::fs::write(root.path().join("dir").join("a.txt"), b"data").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();
    app.cd(Some("dir".to_string())).await.unwrap();

    app.delete("a.txt".to_string(), None, false).await.unwrap();
    assert!(!root.path().join("dir").join("a.txt").exists());
    assert!(root.path().join("dir").exists());
}

#[tokio::test]
async fn delete_targets_another_vault_via_a_vault_prefix() {
    let root1 = tempfile::tempdir().unwrap();
    let root2 = tempfile::tempdir().unwrap();
    std::fs::write(root1.path().join("a.txt"), b"keep").unwrap();
    std::fs::write(root2.path().join("a.txt"), b"drop").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root1.path().to_path_buf()));
    vaults.insert("v2".to_string(), fs_vault("v2", root2.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    app.delete("v2:/a.txt".to_string(), None, false)
        .await
        .unwrap();
    assert!(!root2.path().join("a.txt").exists());
    assert!(root1.path().join("a.txt").exists());
}

#[tokio::test]
async fn delete_of_the_vault_root_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"data").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    assert!(app.delete("/".to_string(), None, true).await.is_err());
    assert!(root.path().join("a.txt").exists());
}

#[tokio::test]
async fn delete_of_a_missing_object_errors() {
    let root = tempfile::tempdir().unwrap();
    let mut vaults = HashMap::new();
    vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();

    assert!(
        app.delete("nope.txt".to_string(), None, false)
            .await
            .is_err()
    );
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
    assert_eq!(
        candidates,
        vec!["docs".to_string(), "downloads".to_string()]
    );

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

// --- regressions: moves are native renames; copies never overwrite without --force ---

/// An fs origin with no native `rename`, so moves take the copy-then-delete fallback. Fails
/// `fetch` for any id ending in `fail_on`, to simulate a copy breaking partway.
struct NoRenameOrigin {
    inner: OriginFileSystem,
    fail_on: Option<&'static str>,
}

#[async_trait::async_trait]
impl nimbus_vault::origin::Origin for NoRenameOrigin {
    async fn fetch(&self, id: &ObjectId) -> nimbus_vault::VaultResult<ByteStream> {
        if let Some(suffix) = self.fail_on
            && id.as_str().ends_with(suffix)
        {
            return Err(VaultError::OriginError("simulated failure".to_string()));
        }
        self.inner.fetch(id).await
    }
    async fn list(&self, id: &ObjectId) -> nimbus_vault::VaultResult<Vec<Object>> {
        self.inner.list(id).await
    }
    async fn get(&self, id: &ObjectId) -> nimbus_vault::VaultResult<Object> {
        self.inner.get(id).await
    }
    async fn put(
        &self,
        object: &mut Object,
        destination: &ObjectId,
    ) -> nimbus_vault::VaultResult<Object> {
        self.inner.put(object, destination).await
    }
    async fn send(&self, object: &Object, payload: ByteStream) -> nimbus_vault::VaultResult<()> {
        self.inner.send(object, payload).await
    }
    async fn delete(&self, id: &ObjectId) -> nimbus_vault::VaultResult<()> {
        self.inner.delete(id).await
    }
}

fn no_rename_vault(name: &str, root: PathBuf, fail_on: Option<&'static str>) -> Vault {
    let origin = NoRenameOrigin {
        inner: OriginFileSystem::new(root),
        fail_on,
    };
    Vault::from_parts(name.to_string(), Arc::new(origin), ObjectId::from("")).unwrap()
}

fn single_vault_app(vault: Vault) -> App {
    let name = vault.get_name().clone();
    let mut vaults = HashMap::new();
    vaults.insert(name.clone(), vault);
    let mut app = make_app(vaults);
    app.select(name).unwrap();
    app
}

#[cfg(unix)]
fn inode(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).unwrap().ino()
}

#[cfg(unix)]
#[tokio::test]
async fn mv_within_a_vault_is_a_native_rename() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("big/nested")).unwrap();
    std::fs::write(root.path().join("big/nested/a.txt"), b"data").unwrap();
    std::fs::create_dir(root.path().join("archive")).unwrap();
    let before = inode(&root.path().join("big"));
    let mut app = single_vault_app(fs_vault("v1", root.path().to_path_buf()));

    app.mv("big".to_string(), "archive".to_string(), None)
        .await
        .unwrap();

    // Same inode: the directory was renamed, not copied and deleted.
    assert_eq!(inode(&root.path().join("archive/big")), before);
    assert!(!root.path().join("big").exists());
    assert_eq!(
        std::fs::read(root.path().join("archive/big/nested/a.txt")).unwrap(),
        b"data"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn rename_is_a_native_rename() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("docs")).unwrap();
    std::fs::write(root.path().join("docs/a.txt"), b"x").unwrap();
    let before = inode(&root.path().join("docs"));
    let mut app = single_vault_app(fs_vault("v1", root.path().to_path_buf()));

    app.rename("docs".to_string(), "papers".to_string(), None)
        .await
        .unwrap();
    assert_eq!(inode(&root.path().join("papers")), before);
    assert!(root.path().join("papers/a.txt").exists());
}

#[tokio::test]
async fn mv_falls_back_to_copy_and_delete_without_a_native_rename() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("docs/deep")).unwrap();
    std::fs::write(root.path().join("docs/deep/a.txt"), b"data").unwrap();
    std::fs::create_dir(root.path().join("archive")).unwrap();
    let mut app = single_vault_app(no_rename_vault("v1", root.path().to_path_buf(), None));

    app.mv("docs".to_string(), "archive".to_string(), None)
        .await
        .unwrap();
    assert!(!root.path().join("docs").exists());
    assert_eq!(
        std::fs::read(root.path().join("archive/docs/deep/a.txt")).unwrap(),
        b"data"
    );
}

#[tokio::test]
async fn a_failed_fallback_move_leaves_the_source_and_no_partial_copy() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("docs")).unwrap();
    std::fs::write(root.path().join("docs/1.txt"), b"one").unwrap();
    std::fs::write(root.path().join("docs/2.txt"), b"two").unwrap();
    std::fs::create_dir(root.path().join("archive")).unwrap();
    let mut app = single_vault_app(no_rename_vault(
        "v1",
        root.path().to_path_buf(),
        Some("2.txt"),
    ));

    let result = app
        .mv("docs".to_string(), "archive".to_string(), None)
        .await;
    assert!(result.is_err());
    // The source is intact and the half-made copy was removed.
    assert_eq!(
        std::fs::read(root.path().join("docs/1.txt")).unwrap(),
        b"one"
    );
    assert_eq!(
        std::fs::read(root.path().join("docs/2.txt")).unwrap(),
        b"two"
    );
    assert!(!root.path().join("archive/docs").exists());
}

#[tokio::test]
async fn cp_into_a_directory_holding_the_same_name_needs_force() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"new").unwrap();
    std::fs::create_dir(root.path().join("dir")).unwrap();
    std::fs::write(root.path().join("dir/a.txt"), b"old").unwrap();
    let mut app = single_vault_app(fs_vault("v1", root.path().to_path_buf()));

    let refused = app
        .cp("a.txt".to_string(), "dir".to_string(), None)
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("--force"), "{refused}");
    assert_eq!(
        std::fs::read(root.path().join("dir/a.txt")).unwrap(),
        b"old"
    );

    app.cp_with("a.txt".to_string(), "dir".to_string(), None, true)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(root.path().join("dir/a.txt")).unwrap(),
        b"new"
    );
}

#[tokio::test]
async fn cp_onto_an_existing_file_path_overwrites_only_with_force() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"new").unwrap();
    std::fs::write(root.path().join("b.txt"), b"old").unwrap();
    let mut app = single_vault_app(fs_vault("v1", root.path().to_path_buf()));

    assert!(
        app.cp("a.txt".to_string(), "b.txt".to_string(), None)
            .await
            .is_err()
    );
    app.cp_with("a.txt".to_string(), "b.txt".to_string(), None, true)
        .await
        .unwrap();
    assert_eq!(std::fs::read(root.path().join("b.txt")).unwrap(), b"new");
}

#[tokio::test]
async fn mv_with_force_replaces_an_existing_file() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"new").unwrap();
    std::fs::create_dir(root.path().join("dir")).unwrap();
    std::fs::write(root.path().join("dir/a.txt"), b"old").unwrap();
    let mut app = single_vault_app(fs_vault("v1", root.path().to_path_buf()));

    assert!(
        app.mv("a.txt".to_string(), "dir".to_string(), None)
            .await
            .is_err()
    );
    assert!(root.path().join("a.txt").exists());
    app.mv_with("a.txt".to_string(), "dir".to_string(), None, true)
        .await
        .unwrap();
    assert!(!root.path().join("a.txt").exists());
    assert_eq!(
        std::fs::read(root.path().join("dir/a.txt")).unwrap(),
        b"new"
    );
}

#[tokio::test]
async fn a_file_never_replaces_a_directory_even_with_force() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("thing"), b"a file").unwrap();
    std::fs::create_dir_all(root.path().join("dir/thing")).unwrap();
    std::fs::write(root.path().join("dir/thing/keep.txt"), b"keep").unwrap();
    let mut app = single_vault_app(fs_vault("v1", root.path().to_path_buf()));

    for force in [false, true] {
        let err = app
            .cp_with("thing".to_string(), "dir".to_string(), None, force)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("is a directory"), "{err}");
    }
    assert_eq!(
        std::fs::read(root.path().join("dir/thing/keep.txt")).unwrap(),
        b"keep"
    );
}

#[tokio::test]
async fn put_and_get_refuse_to_overwrite_without_force() {
    let local_root = tempfile::tempdir().unwrap();
    let remote_root = tempfile::tempdir().unwrap();
    std::fs::write(local_root.path().join("a.txt"), b"local").unwrap();
    std::fs::write(remote_root.path().join("a.txt"), b"remote").unwrap();
    let mut vaults = HashMap::new();
    vaults.insert(
        LOCAL_VAULT_NAME.to_string(),
        fs_vault(LOCAL_VAULT_NAME, local_root.path().to_path_buf()),
    );
    vaults.insert(
        "v1".to_string(),
        fs_vault("v1", remote_root.path().to_path_buf()),
    );
    let mut app = make_app(vaults);
    app.select("v1".to_string()).unwrap();
    app.local_root_canonical = Some(local_root.path().canonicalize().unwrap());
    app.local_root = Some(local_root.path().to_path_buf());

    let local_file = local_root
        .path()
        .join("a.txt")
        .to_string_lossy()
        .into_owned();
    assert!(app.put(local_file.clone(), None, None).await.is_err());
    assert!(app.get("a.txt".to_string(), None, None).await.is_err());
    assert_eq!(
        std::fs::read(remote_root.path().join("a.txt")).unwrap(),
        b"remote"
    );
    assert_eq!(
        std::fs::read(local_root.path().join("a.txt")).unwrap(),
        b"local"
    );

    app.put_with(local_file, None, None, true).await.unwrap();
    assert_eq!(
        std::fs::read(remote_root.path().join("a.txt")).unwrap(),
        b"local"
    );
}

#[tokio::test]
async fn write_object_bytes_stamps_a_new_modification_time() {
    // An edit saved back must look newer than before; reusing the old time (as a copy does)
    // would make push/pull treat the edited file as unchanged or older.
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("a.txt");
    std::fs::write(&path, b"old").unwrap();
    let old_time = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(old_time)
        .unwrap();
    let app = single_vault_app(fs_vault("v1", root.path().to_path_buf()));

    app.write_object_bytes(ObjectId::from("a.txt"), b"edited".to_vec())
        .await
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"edited");
    assert!(std::fs::metadata(&path).unwrap().modified().unwrap() > old_time);
}

// --- regressions: the session file is written atomically and merged, not clobbered ---

fn registry_on_disk(path: &Path) -> HashMap<String, PathBuf> {
    let saved: SavedState = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    saved.vault_configs
}

#[test]
fn two_sessions_registering_vaults_keep_both() {
    // The CLI and TUI open at the same time: each used to overwrite the whole registry, so the
    // vault registered in one vanished when the other saved.
    let dir = tempfile::tempdir().unwrap();
    let shared = dir.path().join("session.toml");
    let a_cfg = write_vault_config(dir.path(), "a.toml", "vault-a");
    let b_cfg = write_vault_config(dir.path(), "b.toml", "vault-b");
    let mut first = make_app(HashMap::new());
    let mut second = make_app(HashMap::new());
    first.state_path = shared.clone();
    second.state_path = shared.clone();

    first.new_vault(a_cfg.clone()).unwrap();
    second.new_vault(b_cfg.clone()).unwrap();
    // Any later save from the first session (e.g. on exit) must not drop vault-b either.
    first.save().unwrap();

    let registry = registry_on_disk(&shared);
    assert_eq!(registry.get("vault-a"), Some(&a_cfg));
    assert_eq!(registry.get("vault-b"), Some(&b_cfg));
}

#[test]
fn a_vault_forgotten_in_one_session_is_not_brought_back_by_another() {
    let dir = tempfile::tempdir().unwrap();
    let shared = dir.path().join("session.toml");
    let cfg = write_vault_config(dir.path(), "a.toml", "vault-a");
    let mut first = make_app(HashMap::new());
    first.state_path = shared.clone();
    first.new_vault(cfg.clone()).unwrap();

    // A second session that started while vault-a was registered...
    let mut second = make_app(HashMap::new());
    second.state_path = shared.clone();
    second.vault_configs.insert("vault-a".to_string(), cfg);
    // ...doesn't resurrect it after the first session forgets it.
    first.forget_vault("vault-a".to_string()).unwrap();
    second.save().unwrap();

    assert!(!registry_on_disk(&shared).contains_key("vault-a"));
}

#[test]
fn a_corrupt_session_file_is_moved_aside_instead_of_blocking_startup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.toml");
    std::fs::write(&path, "vault_configs = { truncated").unwrap();

    let saved = App::load_state(&path).unwrap();
    assert!(saved.vault_configs.is_empty());
    assert!(!path.exists());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("session.toml.bak")).unwrap(),
        "vault_configs = { truncated"
    );
}

#[test]
fn a_missing_session_file_is_a_fresh_session() {
    let dir = tempfile::tempdir().unwrap();
    let saved = App::load_state(&dir.path().join("none.toml")).unwrap();
    assert!(saved.vault_configs.is_empty() && saved.current_vault.is_none());
}

#[cfg(unix)]
#[test]
fn the_session_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let mut app = make_app(HashMap::new());
    app.state_path = dir.path().join("session.toml");
    app.save().unwrap();
    let mode = std::fs::metadata(&app.state_path)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}

// --- regressions: saving an edit back never overwrites a change made meanwhile ---

#[tokio::test]
async fn save_edit_writes_when_nothing_changed_since_opening() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"v1").unwrap();
    let app = single_vault_app(fs_vault("v1", root.path().to_path_buf()));

    let opened = app.fresh_object(ObjectId::from("a.txt")).await.unwrap();
    let outcome = app.save_edit(&opened, b"edited".to_vec()).await.unwrap();
    assert_eq!(outcome, SaveOutcome::Saved);
    assert_eq!(std::fs::read(root.path().join("a.txt")).unwrap(), b"edited");
}

#[tokio::test]
async fn save_edit_refuses_when_the_object_changed_while_editing() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("a.txt");
    std::fs::write(&path, b"v1").unwrap();
    let app = single_vault_app(fs_vault("v1", root.path().to_path_buf()));

    let opened = app.fresh_object(ObjectId::from("a.txt")).await.unwrap();
    // Someone else saves a new version while the editor is open.
    std::fs::write(&path, b"saved elsewhere in the meantime").unwrap();

    let outcome = app.save_edit(&opened, b"my edit".to_vec()).await.unwrap();
    assert_eq!(outcome, SaveOutcome::ChangedMeanwhile);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"saved elsewhere in the meantime"
    );
}

// --- regression: saving an edit keeps the object's metadata, only resetting `modified` ---

#[tokio::test]
async fn write_object_bytes_keeps_per_object_metadata_for_send_cmd() {
    // It used to replace the metadata with `Metadata::new()` plus the size, so a `send_cmd`
    // using the object's own `{bucket}` extra (or `{content_type}`) ran with the literal braces.
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().display();
    let object_json = r#"{"Leaf":{"name":"a.txt","id":"a.txt","meta":{"size":3,"content_type":"text/plain","modified":"2026-01-01T00:00:00Z","extra":{"bucket":"b1"}}}}"#;
    let origin = nimbus_vault::origin::command::OriginCommand::new(
        "true".to_string(),
        "echo []".to_string(),
        format!("echo '{object_json}'"),
        "true".to_string(),
        format!(
            "cat > {d}/out-{{bucket}}; printf %s {{content_type}} > {d}/ct; \
             printf %s {{size}} > {d}/size; printf %s {{modified}} > {d}/modified"
        ),
        "true".to_string(),
        None,
    );
    let vault = Vault::from_parts("c".to_string(), Arc::new(origin), ObjectId::from("")).unwrap();
    let app = single_vault_app(vault);

    app.write_object_bytes(ObjectId::from("a.txt"), b"edited".to_vec())
        .await
        .unwrap();

    assert_eq!(std::fs::read(dir.path().join("out-b1")).unwrap(), b"edited");
    assert!(!dir.path().join("out-{bucket}").exists());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("ct")).unwrap(),
        "text/plain"
    );
    // The size follows the new content; the old modification time is not reused.
    assert_eq!(
        std::fs::read_to_string(dir.path().join("size")).unwrap(),
        "6"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("modified")).unwrap(),
        ""
    );
}

// --- regression: `mv --force` onto an existing directory behaves the same on every origin ---

/// `docs/` (with `a.txt`) and `dest/docs/` (with `b.txt`) in one vault root.
fn two_docs_dirs() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("docs")).unwrap();
    std::fs::write(root.path().join("docs/a.txt"), b"a").unwrap();
    std::fs::create_dir_all(root.path().join("dest/docs")).unwrap();
    std::fs::write(root.path().join("dest/docs/b.txt"), b"b").unwrap();
    root
}

#[tokio::test]
async fn mv_force_refuses_to_replace_a_directory_with_or_without_a_native_rename() {
    // Before: the fs origin's native rename failed with an origin error, while an origin
    // without one merged the two trees and deleted the source.
    for native in [true, false] {
        let root = two_docs_dirs();
        let vault = match native {
            true => fs_vault("v1", root.path().to_path_buf()),
            false => no_rename_vault("v1", root.path().to_path_buf(), None),
        };
        let mut app = single_vault_app(vault);

        let err = app
            .mv_with("docs".to_string(), "dest".to_string(), None, true)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("can't replace a directory"),
            "{err}"
        );
        // Nothing moved or merged.
        assert!(root.path().join("docs/a.txt").exists(), "native={native}");
        assert!(
            !root.path().join("dest/docs/a.txt").exists(),
            "native={native}"
        );
        assert!(
            root.path().join("dest/docs/b.txt").exists(),
            "native={native}"
        );
    }
}

#[tokio::test]
async fn cp_force_merges_into_an_existing_directory_on_every_origin() {
    for native in [true, false] {
        let root = two_docs_dirs();
        let vault = match native {
            true => fs_vault("v1", root.path().to_path_buf()),
            false => no_rename_vault("v1", root.path().to_path_buf(), None),
        };
        let mut app = single_vault_app(vault);

        app.cp_with("docs".to_string(), "dest".to_string(), None, true)
            .await
            .unwrap();
        assert!(
            root.path().join("dest/docs/a.txt").exists(),
            "native={native}"
        );
        assert!(
            root.path().join("dest/docs/b.txt").exists(),
            "native={native}"
        );
        assert!(root.path().join("docs/a.txt").exists(), "native={native}");
    }
}

#[tokio::test]
async fn mv_force_still_replaces_a_file() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"new").unwrap();
    std::fs::create_dir(root.path().join("dest")).unwrap();
    std::fs::write(root.path().join("dest/a.txt"), b"old").unwrap();
    for native in [true, false] {
        std::fs::write(root.path().join("a.txt"), b"new").unwrap();
        std::fs::write(root.path().join("dest/a.txt"), b"old").unwrap();
        let vault = match native {
            true => fs_vault("v1", root.path().to_path_buf()),
            false => no_rename_vault("v1", root.path().to_path_buf(), None),
        };
        let mut app = single_vault_app(vault);
        app.mv_with("a.txt".to_string(), "dest".to_string(), None, true)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(root.path().join("dest/a.txt")).unwrap(),
            b"new"
        );
        assert!(!root.path().join("a.txt").exists(), "native={native}");
    }
}

// --- session file location ---

#[test]
fn session_path_prefers_the_state_dir() {
    let path = App::session_path(Some("/state".into()), Some("/data".into())).unwrap();
    assert_eq!(path, PathBuf::from("/state/nimbus/session.toml"));
}

#[test]
fn session_path_falls_back_to_the_local_data_dir() {
    let path = App::session_path(None, Some("/data".into())).unwrap();
    assert_eq!(path, PathBuf::from("/data/nimbus/session.toml"));
}

#[test]
fn session_path_refuses_to_guess_without_either_dir() {
    assert!(App::session_path(None, None).is_err());
}

#[test]
fn legacy_session_path_is_under_the_temp_dir_without_a_state_dir() {
    assert_eq!(
        App::legacy_session_path(None, Path::new("/tmp")),
        Some(PathBuf::from("/tmp/nimbus/session.toml"))
    );
}

#[test]
fn a_legacy_session_file_is_ignored_when_there_is_a_state_dir() {
    // No older version wrote to the temp dir on such a system, so a file there is someone
    // else's: it must not even be looked at.
    assert_eq!(
        App::legacy_session_path(Some(Path::new("/state")), Path::new("/tmp")),
        None
    );
}

/// A legacy and a new session path in one scratch dir; the new one's parent doesn't exist yet.
fn session_paths(dir: &Path) -> (PathBuf, PathBuf) {
    let legacy = dir.join("tmp/nimbus/session.toml");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    (dir.join("state/nimbus/session.toml"), legacy)
}

#[test]
fn migrating_moves_a_legacy_session_file_and_makes_it_private() {
    let dir = tempfile::tempdir().unwrap();
    let (new, legacy) = session_paths(dir.path());
    std::fs::write(&legacy, "current_vault = \"old\"\n").unwrap();

    let outcome = App::migrate_legacy_session(&new, &legacy).unwrap();

    assert_eq!(outcome, LegacySession::Moved);
    assert!(!legacy.exists());
    assert_eq!(
        std::fs::read_to_string(&new).unwrap(),
        "current_vault = \"old\"\n"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&new).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[test]
fn migrating_keeps_the_new_session_file_when_both_exist() {
    let dir = tempfile::tempdir().unwrap();
    let (new, legacy) = session_paths(dir.path());
    std::fs::create_dir_all(new.parent().unwrap()).unwrap();
    std::fs::write(&legacy, "current_vault = \"old\"\n").unwrap();
    std::fs::write(&new, "current_vault = \"new\"\n").unwrap();

    let outcome = App::migrate_legacy_session(&new, &legacy).unwrap();

    assert_eq!(outcome, LegacySession::KeptNew);
    assert_eq!(
        std::fs::read_to_string(&new).unwrap(),
        "current_vault = \"new\"\n"
    );
    assert_eq!(
        std::fs::read_to_string(&legacy).unwrap(),
        "current_vault = \"old\"\n"
    );
}

#[test]
fn migrating_does_nothing_without_a_legacy_session_file() {
    let dir = tempfile::tempdir().unwrap();
    let (new, legacy) = session_paths(dir.path());

    let outcome = App::migrate_legacy_session(&new, &legacy).unwrap();

    assert_eq!(outcome, LegacySession::None);
    assert!(!new.exists());
}

/// Asserts that migrating refuses the legacy file, leaving it (and the new path) untouched.
#[cfg(unix)]
fn assert_migration_refused(new: &Path, legacy: &Path, why: &str) {
    let before = std::fs::symlink_metadata(legacy).unwrap();
    match App::migrate_legacy_session(new, legacy).unwrap() {
        LegacySession::Refused(reason) => assert!(reason.contains(why), "{reason}"),
        other => panic!("expected a refusal ({why}), got {other:?}"),
    }
    assert!(!new.exists());
    let after = std::fs::symlink_metadata(legacy).unwrap();
    assert_eq!(before.file_type(), after.file_type());
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
#[test]
fn a_group_or_world_writable_legacy_session_file_is_refused() {
    for mode in [0o620, 0o602, 0o666] {
        let dir = tempfile::tempdir().unwrap();
        let (new, legacy) = session_paths(dir.path());
        std::fs::write(&legacy, "current_vault = \"planted\"\n").unwrap();
        set_mode(&legacy, mode);
        assert_migration_refused(&new, &legacy, "writable by other users");
    }
}

#[cfg(unix)]
#[test]
fn a_symlinked_legacy_session_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (new, legacy) = session_paths(dir.path());
    let target = dir.path().join("elsewhere.toml");
    std::fs::write(&target, "current_vault = \"planted\"\n").unwrap();
    std::os::unix::fs::symlink(&target, &legacy).unwrap();
    assert_migration_refused(&new, &legacy, "not a regular file");
}

#[cfg(unix)]
#[test]
fn a_legacy_session_file_in_a_writable_directory_is_refused() {
    for mode in [0o775, 0o777, 0o1777] {
        let dir = tempfile::tempdir().unwrap();
        let (new, legacy) = session_paths(dir.path());
        std::fs::write(&legacy, "current_vault = \"planted\"\n").unwrap();
        set_mode(legacy.parent().unwrap(), mode);
        assert_migration_refused(&new, &legacy, "writable by other users");
    }
}

#[cfg(unix)]
#[test]
fn a_symlinked_legacy_session_directory_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    std::fs::create_dir(&real).unwrap();
    std::fs::write(real.join("session.toml"), "current_vault = \"planted\"\n").unwrap();
    std::fs::create_dir(dir.path().join("tmp")).unwrap();
    std::os::unix::fs::symlink(&real, dir.path().join("tmp/nimbus")).unwrap();
    let new = dir.path().join("state/nimbus/session.toml");
    let legacy = dir.path().join("tmp/nimbus/session.toml");
    assert_migration_refused(&new, &legacy, "not a directory");
}

// A file owned by another uid can't be created without root, so ownership is checked through
// `check_owned_entry` directly, with an owner that isn't the file's.
#[cfg(unix)]
#[test]
fn a_legacy_session_file_owned_by_another_user_is_refused() {
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("session.toml");
    std::fs::write(&file, "").unwrap();
    let mine = std::fs::metadata(&file).unwrap().uid();

    assert!(App::check_owned_entry(&file, false, mine).is_ok());
    let error = App::check_owned_entry(&file, false, mine.wrapping_add(1)).unwrap_err();
    assert!(error.contains("owned by uid"), "{error}");
    let error = App::check_owned_entry(dir.path(), true, mine.wrapping_add(1)).unwrap_err();
    assert!(error.contains("owned by uid"), "{error}");
}
