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
