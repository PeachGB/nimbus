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
