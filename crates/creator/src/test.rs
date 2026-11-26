use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::App;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn type_str(app: &mut App, s: &str) {
    for c in s.chars() {
        app.handle_key_event(key(KeyCode::Char(c))).unwrap();
    }
}

#[test]
fn wizard_builds_and_saves_an_fs_vault() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let config_path = dir.path().join("vault.toml");

    let mut app = App::new();

    // name
    type_str(&mut app, "test-vault");
    app.handle_key_event(key(KeyCode::Enter)).unwrap();

    // root id: leave blank, defaults to "/"
    app.handle_key_event(key(KeyCode::Enter)).unwrap();

    // select origin: Fs is the first entry, just confirm
    app.handle_key_event(key(KeyCode::Enter)).unwrap();

    // field: root directory
    type_str(&mut app, data_dir.to_str().unwrap());
    app.handle_key_event(key(KeyCode::Enter)).unwrap();

    // encrypt at rest? no
    app.handle_key_event(key(KeyCode::Char('n'))).unwrap();

    // save path: clear the auto-filled default and type the real one
    for _ in 0..app_input_len(&app) {
        app.handle_key_event(key(KeyCode::Backspace)).unwrap();
    }
    type_str(&mut app, config_path.to_str().unwrap());
    app.handle_key_event(key(KeyCode::Enter)).unwrap();

    // confirm + save
    app.handle_key_event(key(KeyCode::Enter)).unwrap();

    assert!(config_path.is_file());

    let vault = nimbus_vault::vault::Vault::new(config_path).unwrap();
    assert_eq!(vault.get_name(), "test-vault");
}

fn app_input_len(app: &App) -> usize {
    app.input.len()
}

#[test]
fn tab_completes_a_unique_fs_path_match() {
    use crate::{app::Step, builder::OriginKind};

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("uniquedir");
    std::fs::create_dir_all(&target).unwrap();

    let mut app = App::new();
    app.step = Step::Field(0);
    app.fields = OriginKind::Fs.fields();
    app.origin_kind = Some(OriginKind::Fs);

    let prefix = dir.path().join("uniq");
    type_str(&mut app, prefix.to_str().unwrap());
    app.handle_key_event(key(KeyCode::Tab)).unwrap();

    let expected = format!("{}/", target.to_str().unwrap());
    assert_eq!(app.input, expected);
}

#[test]
fn tab_completes_ambiguous_fs_path_to_common_prefix_and_lists_suggestions() {
    use crate::{app::Step, builder::OriginKind};

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("alpha")).unwrap();
    std::fs::create_dir_all(dir.path().join("alphabet")).unwrap();

    let mut app = App::new();
    app.step = Step::Field(0);
    app.fields = OriginKind::Fs.fields();
    app.origin_kind = Some(OriginKind::Fs);

    let prefix = dir.path().join("al");
    type_str(&mut app, prefix.to_str().unwrap());
    app.handle_key_event(key(KeyCode::Tab)).unwrap();

    let expected_common = dir.path().join("alpha");
    assert_eq!(app.input, expected_common.to_str().unwrap());
    assert_eq!(app.suggestions.len(), 2);
}

#[test]
fn tab_with_no_matches_reports_an_error_instead_of_doing_nothing_silently() {
    use crate::{app::Step, builder::OriginKind};

    let dir = tempfile::tempdir().unwrap();

    let mut app = App::new();
    app.step = Step::Field(0);
    app.fields = OriginKind::Fs.fields();
    app.origin_kind = Some(OriginKind::Fs);

    let prefix = dir.path().join("does-not-exist");
    type_str(&mut app, prefix.to_str().unwrap());
    app.handle_key_event(key(KeyCode::Tab)).unwrap();

    assert_eq!(app.input, prefix.to_str().unwrap());
    assert!(app.error.is_some());
}

#[test]
fn tab_expands_a_leading_tilde_to_the_home_directory() {
    use crate::{app::Step, builder::OriginKind};

    let Some(home) = dirs::home_dir() else {
        return; // no home dir resolvable in this environment; nothing to assert
    };
    let Ok(mut entries) = std::fs::read_dir(&home) else {
        return;
    };
    let Some(Ok(first_entry)) = entries.find(|e| {
        e.as_ref()
            .ok()
            .map(|e| !e.file_name().is_empty())
            .unwrap_or(false)
    }) else {
        return; // empty home dir; nothing to complete against
    };
    let entry_name = first_entry.file_name().to_string_lossy().into_owned();
    let prefix_len = entry_name.chars().count().clamp(1, 3);
    let prefix: String = entry_name.chars().take(prefix_len).collect();

    let mut app = App::new();
    app.step = Step::Field(0);
    app.fields = OriginKind::Fs.fields();
    app.origin_kind = Some(OriginKind::Fs);

    type_str(&mut app, &format!("~/{prefix}"));
    app.handle_key_event(key(KeyCode::Tab)).unwrap();

    assert!(
        app.input.starts_with(home.to_str().unwrap()),
        "expected '{}' to expand under home dir '{}'",
        app.input,
        home.display()
    );
}

/// Drives the wizard as far as the save-path step, building an `fs` vault called `name`.
fn wizard_to_save_path(name: &str, data_dir: &std::path::Path) -> App {
    let mut app = App::new();
    type_str(&mut app, name);
    app.handle_key_event(key(KeyCode::Enter)).unwrap(); // name
    app.handle_key_event(key(KeyCode::Enter)).unwrap(); // root id: blank
    app.handle_key_event(key(KeyCode::Enter)).unwrap(); // origin: Fs (first entry)
    type_str(&mut app, data_dir.to_str().unwrap());
    app.handle_key_event(key(KeyCode::Enter)).unwrap(); // root directory
    app.handle_key_event(key(KeyCode::Char('n'))).unwrap(); // encrypt: no
    app
}

fn retype_save_path(app: &mut App, path: &std::path::Path) {
    for _ in 0..app_input_len(app) {
        app.handle_key_event(key(KeyCode::Backspace)).unwrap();
    }
    type_str(app, path.to_str().unwrap());
    app.handle_key_event(key(KeyCode::Enter)).unwrap();
}

#[test]
fn save_path_defaults_to_the_conventional_vaults_directory() {
    let dir = tempfile::tempdir().unwrap();
    let app = wizard_to_save_path("defaulted", dir.path());

    let expected = nimbus_vault::config::VaultConfig::default_path("defaulted");
    assert_eq!(app.save_path, expected.display().to_string());
    // Offered in the editable field too, not just recorded.
    assert_eq!(app.input, app.save_path);
}

#[test]
fn saving_over_an_existing_config_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let occupied = dir.path().join("taken.toml");
    std::fs::write(&occupied, "name = \"someone-else\"\n").unwrap();

    let mut app = wizard_to_save_path("newcomer", dir.path());
    retype_save_path(&mut app, &occupied);
    app.handle_key_event(key(KeyCode::Enter)).unwrap(); // confirm

    assert!(app.error.is_some(), "expected a refusal");
    // The existing config survives untouched...
    assert_eq!(
        std::fs::read_to_string(&occupied).unwrap(),
        "name = \"someone-else\"\n"
    );
    // ...and the wizard is still running, back on the save-path step, so nothing typed is lost.
    assert!(app.is_running());
    assert_eq!(app.step, crate::app::Step::SavePath);
}

#[test]
fn a_refused_save_can_be_retried_at_another_path() {
    let dir = tempfile::tempdir().unwrap();
    let occupied = dir.path().join("taken.toml");
    std::fs::write(&occupied, "name = \"someone-else\"\n").unwrap();
    let free = dir.path().join("free.toml");

    let mut app = wizard_to_save_path("newcomer", dir.path());
    retype_save_path(&mut app, &occupied);
    app.handle_key_event(key(KeyCode::Enter)).unwrap(); // refused

    retype_save_path(&mut app, &free);
    app.handle_key_event(key(KeyCode::Enter)).unwrap(); // confirm

    assert!(free.is_file());
    let vault = nimbus_vault::vault::Vault::new(free).unwrap();
    assert_eq!(vault.get_name(), "newcomer");
}

#[test]
fn a_relative_path_answer_is_saved_absolute_against_the_cwd() {
    use crate::builder::OriginKind;
    use nimbus_vault::config::OriginConfig;
    use std::collections::HashMap;

    let cwd = std::env::current_dir().unwrap();
    let answers = |key: &str, value: &str| HashMap::from([(key.to_string(), value.to_string())]);

    // The prompt completes against the cwd, so that is what a relative answer means; stored
    // as typed, it would resolve against the saved config's directory instead.
    match OriginKind::Fs.build(&answers("root", "data")) {
        OriginConfig::Fs { root } => assert_eq!(root, cwd.join("data")),
        _ => panic!("expected an fs config"),
    }
    match OriginKind::Vault.build(&answers("path", "inner.toml")) {
        OriginConfig::Vault { path } => assert_eq!(path, cwd.join("inner.toml")),
        _ => panic!("expected a vault config"),
    }
}

#[test]
fn a_tilde_path_answer_is_saved_under_the_home_directory() {
    use crate::builder::OriginKind;
    use nimbus_vault::config::OriginConfig;
    use std::collections::HashMap;

    let Some(home) = dirs::home_dir() else {
        return; // nothing to expand to on this system
    };
    let answers = |value: &str| HashMap::from([("root".to_string(), value.to_string())]);
    let root_for = |value: &str| match OriginKind::Fs.build(&answers(value)) {
        OriginConfig::Fs { root } => root,
        _ => panic!("expected an fs config"),
    };

    // Typed without Tab: the same expansion completion would have done.
    assert_eq!(root_for("~/vault-data"), home.join("vault-data"));
    assert_eq!(root_for("~"), home);
    // `~user` is another user's home, not this one's: left to the cwd rule like any name.
    let cwd = std::env::current_dir().unwrap();
    assert_eq!(root_for("~other/x"), cwd.join("~other/x"));
}

// --- encrypted vaults: the key is shown once and written only after the backup phrase ---

/// Clears the input field and types `text` in its place.
fn retype(app: &mut App, text: &str) {
    for _ in 0..app_input_len(app) {
        app.handle_key_event(key(KeyCode::Backspace)).unwrap();
    }
    type_str(app, text);
}

/// Answers the current text prompt with `text`.
fn answer(app: &mut App, text: &str) {
    retype(app, text);
    app.handle_key_event(key(KeyCode::Enter)).unwrap();
}

/// The files an encrypted-vault test works with, all inside one temp dir.
struct Paths {
    _dir: tempfile::TempDir,
    data: std::path::PathBuf,
    key: std::path::PathBuf,
    config: std::path::PathBuf,
}

fn paths() -> Paths {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    Paths {
        key: dir.path().join("keys").join("secret.key"),
        config: dir.path().join("vault.toml"),
        data,
        _dir: dir,
    }
}

/// Drives the wizard through an encrypted fs vault to the key-backup screen: `y` at Encrypt,
/// then the key path, the config path, and the confirm.
fn wizard_to_key_backup(paths: &Paths) -> App {
    let mut app = App::new();
    answer(&mut app, "secret-vault"); // name
    app.handle_key_event(key(KeyCode::Enter)).unwrap(); // root id: blank
    app.handle_key_event(key(KeyCode::Enter)).unwrap(); // origin: Fs
    answer(&mut app, paths.data.to_str().unwrap()); // root directory
    app.handle_key_event(key(KeyCode::Char('y'))).unwrap(); // encrypt: yes
    answer(&mut app, paths.key.to_str().unwrap()); // key path
    answer(&mut app, paths.config.to_str().unwrap()); // save path
    app.handle_key_event(key(KeyCode::Enter)).unwrap(); // confirm
    assert_eq!(app.step, crate::app::Step::KeyBackup, "{:?}", app.error);
    app
}

/// The rows of the screen the wizard draws right now on a `width` x `height` test terminal.
fn screen_rows(app: &App, width: u16, height: u16) -> Vec<String> {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| crate::ui::render(app, frame))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let symbols: Vec<&str> = buffer.content.iter().map(|cell| cell.symbol()).collect();
    symbols
        .chunks(width as usize)
        .map(|row| row.concat())
        .collect()
}

/// The key text on the screen the wizard draws right now, read back from a test terminal.
fn displayed_key(app: &App) -> String {
    screen_rows(app, 200, 40)
        .into_iter()
        .find_map(|row| {
            let start = row.find("nimbus-crypt-key-1:")?;
            Some(row[start..].split_whitespace().next()?.to_string())
        })
        .expect("the key-backup screen shows the key")
}

fn type_backup_phrase(app: &mut App) {
    answer(app, crate::app::BACKUP_PHRASE);
}

/// The text form of the key in `path`.
fn key_in_file(path: &std::path::Path) -> String {
    let key = nimbus_vault::crypt::CryptKey::load(Some(path), None).unwrap();
    key.to_text().to_string()
}

#[test]
fn answering_no_at_encrypt_saves_a_plain_config_and_no_key() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("plain.toml");
    let name = format!("plain-{}", std::process::id());

    let mut app = wizard_to_save_path(&name, dir.path());
    retype_save_path(&mut app, &config_path);
    app.handle_key_event(key(KeyCode::Enter)).unwrap(); // confirm

    let text = std::fs::read_to_string(&config_path).unwrap();
    assert!(text.contains("type = \"fs\""), "{text}");
    assert!(!text.contains("crypt"), "{text}");
    // The real config home isn't the test's to inspect wholesale, so check this vault's own
    // default key path: nothing was written there.
    let default_key = nimbus_vault::config_home()
        .join("keys")
        .join(format!("{name}.key"));
    assert!(!default_key.exists());
}

#[test]
fn the_full_encrypted_flow_writes_a_private_key_and_a_crypt_config() {
    let paths = paths();
    let mut app = wizard_to_key_backup(&paths);
    let shown = displayed_key(&app);

    type_backup_phrase(&mut app);

    assert!(!app.is_running(), "{:?}", app.error);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&paths.key).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let text = std::fs::read_to_string(&paths.config).unwrap();
    assert!(!text.contains("nimbus-crypt-key-1:"), "{text}");
    let table: toml::Table = toml::from_str(&text).unwrap();
    let origin_config: nimbus_vault::config::OriginConfig =
        table["origin_config"].clone().try_into().unwrap();
    match origin_config {
        nimbus_vault::config::OriginConfig::Crypt {
            key_file, key_env, ..
        } => {
            assert_eq!(key_file.as_deref(), Some(paths.key.as_path()));
            assert!(paths.key.is_absolute());
            assert_eq!(key_env, None);
        }
        _ => panic!("expected a crypt config: {text}"),
    }
    nimbus_vault::vault::Vault::new(paths.config.clone()).unwrap();
    assert_eq!(key_in_file(&paths.key), shown);
}

#[test]
fn nothing_is_written_before_the_backup_phrase() {
    let paths = paths();
    let _app = wizard_to_key_backup(&paths);

    assert!(!paths.key.exists());
    assert!(!paths.config.exists());
}

#[test]
fn a_wrong_backup_phrase_is_refused_and_writes_nothing() {
    let paths = paths();
    let mut app = wizard_to_key_backup(&paths);

    answer(&mut app, "sure, whatever");

    assert!(app.error.is_some());
    assert_eq!(app.step, crate::app::Step::KeyBackup);
    assert!(!paths.key.exists());
    assert!(!paths.config.exists());
}

#[test]
fn esc_at_key_backup_writes_nothing_and_stops() {
    let paths = paths();
    let mut app = wizard_to_key_backup(&paths);

    app.handle_key_event(key(KeyCode::Esc)).unwrap();

    assert!(!app.is_running());
    assert!(app.into_outcome().is_none());
    assert!(!paths.key.exists());
    assert!(!paths.config.exists());
}

#[test]
fn a_key_path_collision_keeps_the_same_key_and_the_existing_file() {
    let paths = paths();
    std::fs::create_dir_all(paths.key.parent().unwrap()).unwrap();
    std::fs::write(&paths.key, "someone else's key\n").unwrap();
    let free = paths.key.with_file_name("free.key");
    let mut app = wizard_to_key_backup(&paths);
    let shown = displayed_key(&app);

    type_backup_phrase(&mut app);

    assert_eq!(app.step, crate::app::Step::KeyPath);
    assert!(app.error.as_deref().unwrap().contains("already exists"));
    assert_eq!(
        std::fs::read_to_string(&paths.key).unwrap(),
        "someone else's key\n"
    );
    assert!(!paths.config.exists());

    answer(&mut app, free.to_str().unwrap()); // key path
    app.handle_key_event(key(KeyCode::Enter)).unwrap(); // save path: the earlier answer
    app.handle_key_event(key(KeyCode::Enter)).unwrap(); // confirm
    assert_eq!(app.step, crate::app::Step::KeyBackup);
    assert_eq!(displayed_key(&app), shown);

    type_backup_phrase(&mut app);

    assert!(!app.is_running(), "{:?}", app.error);
    assert_eq!(key_in_file(&free), shown);
}

/// `paths` with the config under a file where its directory should be: the key writes fine,
/// the config save can't. Also returns that file, whose sibling `vault.toml` saves fine.
fn unsavable(paths: Paths) -> (Paths, std::path::PathBuf) {
    let blocker = paths.config.with_file_name("blocker");
    std::fs::write(&blocker, "").unwrap();
    let paths = Paths {
        config: blocker.join("vault.toml"),
        ..paths
    };
    (paths, blocker)
}

#[test]
fn a_config_save_failure_keeps_the_written_key_and_a_retry_does_not_rewrite_it() {
    let (unsavable, blocker) = unsavable(paths());
    let mut app = wizard_to_key_backup(&unsavable);
    let shown = displayed_key(&app);

    type_backup_phrase(&mut app);

    assert_eq!(app.step, crate::app::Step::SavePath);
    let error = app.error.clone().unwrap();
    assert!(
        error.contains(&unsavable.key.display().to_string()),
        "{error}"
    );
    assert_eq!(key_in_file(&unsavable.key), shown);
    let written = std::fs::metadata(&unsavable.key)
        .unwrap()
        .modified()
        .unwrap();

    let good = blocker.with_file_name("vault.toml");
    answer(&mut app, good.to_str().unwrap()); // save path
    app.handle_key_event(key(KeyCode::Enter)).unwrap(); // confirm

    assert!(!app.is_running(), "{:?}", app.error);
    assert!(good.is_file());
    assert_eq!(key_in_file(&unsavable.key), shown);
    let after = std::fs::metadata(&unsavable.key)
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(written, after);
}

#[test]
fn a_tilde_key_path_answer_is_saved_under_the_home_directory() {
    let Some(home) = dirs::home_dir() else {
        return; // nothing to expand to on this system
    };
    let mut app = App::new();
    app.key_path = "~/keys/v.key".to_string();
    assert_eq!(app.key_file(), home.join("keys/v.key"));
}

/// `paths()`, with the key in a directory long enough that the key path is over 60 characters.
fn paths_with_a_long_key_path() -> Paths {
    let paths = paths();
    let key = paths
        .key
        .parent()
        .unwrap()
        .join("for-a-vault-whose-key-directory-has-a-deliberately-long-name")
        .join("secret.key");
    assert!(key.to_str().unwrap().len() >= 60);
    Paths { key, ..paths }
}

/// Asserts that the wizard's current error appears whole on an 80-column screen, once the
/// wrapped footer lines are joined (whitespace aside: a path splits across lines anywhere).
fn assert_error_shown_whole_at_80_columns(app: &App) {
    let error = format!("error: {}", app.error.as_deref().expect("an error"));
    let squeeze = |text: &str| text.split_whitespace().collect::<String>();
    let screen = squeeze(&screen_rows(app, 80, 30).concat());
    assert!(screen.contains(&squeeze(&error)), "{error}");
}

#[test]
fn a_long_key_path_collision_error_is_shown_whole() {
    let paths = paths_with_a_long_key_path();
    std::fs::create_dir_all(paths.key.parent().unwrap()).unwrap();
    std::fs::write(&paths.key, "someone else's key\n").unwrap();
    let mut app = wizard_to_key_backup(&paths);

    type_backup_phrase(&mut app);

    assert_eq!(app.step, crate::app::Step::KeyPath);
    assert_error_shown_whole_at_80_columns(&app);
}

#[test]
fn a_long_save_failure_error_is_shown_whole() {
    let (paths, _) = unsavable(paths_with_a_long_key_path());
    let mut app = wizard_to_key_backup(&paths);

    type_backup_phrase(&mut app);

    assert_eq!(app.step, crate::app::Step::SavePath);
    assert_error_shown_whole_at_80_columns(&app);
}

#[test]
fn esc_after_a_save_failure_reports_the_kept_key_file() {
    let (paths, _) = unsavable(paths());
    let mut app = wizard_to_key_backup(&paths);
    type_backup_phrase(&mut app); // the key is written, the config save fails

    app.handle_key_event(key(KeyCode::Esc)).unwrap();

    assert!(paths.key.is_file());
    assert_eq!(app.unsaved_key_file(), Some(paths.key.as_path()));
    let outcome = app.into_result();
    assert_eq!(outcome.config, None);
    assert_eq!(outcome.unsaved_key_file, Some(paths.key.clone()));
}

#[test]
fn a_saved_wizard_reports_no_unsaved_key_file() {
    let paths = paths();
    let mut app = wizard_to_key_backup(&paths);

    type_backup_phrase(&mut app);

    assert_eq!(app.unsaved_key_file(), None);
    let outcome = app.into_result();
    assert_eq!(outcome.config, Some(paths.config.clone()));
    assert_eq!(outcome.unsaved_key_file, None);
}

/// The `key_file` the config at `path` names.
fn saved_key_file(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let table: toml::Table = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    match table["origin_config"].clone().try_into().unwrap() {
        nimbus_vault::config::OriginConfig::Crypt { key_file, .. } => key_file,
        _ => panic!("expected a crypt config"),
    }
}

#[test]
fn the_config_names_the_key_file_actually_written() {
    let (paths, blocker) = unsavable(paths());
    let mut app = wizard_to_key_backup(&paths);
    type_backup_phrase(&mut app); // the key is written, the config save fails
    // Nothing in the wizard edits the key path after this, so the answer is changed directly:
    // whatever happens to it, the config must name the file that was written.
    app.key_path = "/somewhere/else.key".to_string();

    let good = blocker.with_file_name("vault.toml");
    answer(&mut app, good.to_str().unwrap()); // save path
    app.handle_key_event(key(KeyCode::Enter)).unwrap(); // confirm

    assert!(!app.is_running(), "{:?}", app.error);
    assert_eq!(saved_key_file(&good), Some(paths.key.clone()));
}

#[cfg(unix)]
fn mode_of(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[cfg(unix)]
#[test]
fn a_new_nested_keys_dir_is_private() {
    let paths = paths();
    let base = paths.key.parent().unwrap().parent().unwrap().to_path_buf();
    let base_mode = mode_of(&base);
    let keys = base.join("a").join("b").join("keys");
    let nested = Paths {
        key: keys.join("secret.key"),
        ..paths
    };
    let mut app = wizard_to_key_backup(&nested);

    type_backup_phrase(&mut app);

    assert!(nested.key.is_file(), "{:?}", app.error);
    for created in [base.join("a"), base.join("a/b"), keys] {
        // Owner-only; the umask can only have narrowed the mkdir's 0700.
        assert_eq!(mode_of(&created) & 0o077, 0, "{}", created.display());
    }
    assert_eq!(mode_of(&base), base_mode);
}

#[cfg(unix)]
#[test]
fn an_existing_keys_dir_keeps_its_mode() {
    use std::os::unix::fs::PermissionsExt;
    let paths = paths();
    let keys = paths.key.parent().unwrap();
    std::fs::create_dir_all(keys).unwrap();
    std::fs::set_permissions(keys, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut app = wizard_to_key_backup(&paths);

    type_backup_phrase(&mut app);

    assert!(paths.key.is_file(), "{:?}", app.error);
    assert_eq!(mode_of(keys), 0o755);
}
