use super::*;

#[derive(serde::Deserialize, Debug)]
#[allow(dead_code)]
struct Auth {
    token: String,
    letter: Option<char>,
}

#[derive(serde::Deserialize, Debug)]
#[allow(dead_code)]
#[serde(deny_unknown_fields)]
struct Config {
    name: String,
    auth: Auth,
}

/// Parses `raw` as a [`Config`] from `vault.toml`, expecting it to fail.
fn parse_error(raw: &str) -> ConfigParseError {
    parse_toml::<Config>(raw, Path::new("vault.toml")).unwrap_err()
}

/// Asserts that `secret` shows up in none of the ways an error gets printed.
fn assert_hidden(error: &ConfigParseError, secret: &str) {
    for shown in [
        error.to_string(),
        format!("{error:?}"),
        format!("{error:#}"),
        format!("{error:#?}"),
    ] {
        assert!(!shown.contains(secret), "{shown}");
    }
}

#[test]
fn a_syntax_error_names_the_place_but_not_the_line() {
    let error = parse_error("name = \"v\"\n\n[auth]\ntoken = \"SUPERSECRET\" extra\n");
    assert_eq!(error.kind, ParseErrorKind::Syntax);
    assert_eq!(error.line, Some(4));
    assert!(
        error
            .to_string()
            .starts_with("vault.toml: syntax error at line 4"),
        "{error}"
    );
    assert_hidden(&error, "SUPERSECRET");
}

#[test]
fn a_wrong_type_does_not_quote_the_value() {
    let error = parse_error("name = \"v\"\n\n[auth]\ntoken = 12345678\n");
    assert_eq!(error.kind, ParseErrorKind::WrongType);
    assert_eq!(
        error.to_string(),
        "vault.toml: wrong type at line 4, column 9"
    );
    assert_hidden(&error, "12345678");
}

#[test]
fn an_invalid_quoted_value_is_not_quoted_back() {
    // serde's message here is `invalid value: string "SUPERSECRET", expected a character`.
    let error = parse_error("name = \"v\"\n\n[auth]\ntoken = \"t\"\nletter = \"SUPERSECRET\"\n");
    assert_eq!(error.kind, ParseErrorKind::InvalidValue);
    assert_hidden(&error, "SUPERSECRET");
}

#[test]
fn only_a_missing_field_is_named_because_its_name_comes_from_the_schema() {
    let error = parse_error("name = \"v\"\nnmae = \"x\"\n\n[auth]\ntoken = \"t\"\n");
    assert_eq!(error.kind, ParseErrorKind::UnknownField);
    assert!(!error.to_string().contains("nmae"), "{error}");

    let error = parse_error("name = \"v\"\n\n[auth]\nletter = \"a\"\n");
    assert_eq!(
        error.kind,
        ParseErrorKind::MissingField("token".to_string())
    );
}

#[test]
fn position_counts_lines_and_characters_from_one() {
    assert_eq!(position_of("a = 1\nbé = 2\n", 0), (1, 1));
    assert_eq!(position_of("a = 1\nbé = 2\n", 6), (2, 1));
    // Past the two-byte `é`: column 3, not 4.
    assert_eq!(position_of("a = 1\nbé = 2\n", 9), (2, 3));
}

// --- multi-line strings: their lines look like `key = …` or `[table]` but are the value ---

/// An http vault config whose `[origin_config.auth]` table ends with `auth_tail`.
fn vault_config_with_auth(auth_tail: &str) -> String {
    format!(
        "name = \"remote\"\n\n[origin_config]\ntype = \"http\"\nlist_url = \"/l\"\n\
         fetch_url = \"/f\"\nget_url = \"/g\"\nput_url = \"/p\"\nsend_url = \"/s\"\n\
         delete_url = \"/d\"\n\n[origin_config.auth]\ntype = \"bearer\"\n{auth_tail}"
    )
}

/// Asserts that loading `auth_tail` as a vault config fails without "SECRET" in any rendering.
fn assert_secret_not_echoed(auth_tail: &str) {
    let raw = vault_config_with_auth(auth_tail);
    let error = match parse_toml::<crate::config::VaultConfig>(&raw, Path::new("vault.toml")) {
        Ok(_) => panic!("{auth_tail:?} should not load"),
        Err(error) => error,
    };
    assert_hidden(&error, "SECRET");
}

#[test]
fn ml_ctrl_a_control_character_in_a_multiline_string_is_not_echoed() {
    assert_secret_not_echoed("token = \"\"\"\nabcDEF123SECRET==\u{7f}\n\"\"\"\n");
}

#[test]
fn ml_ctrl_lit_a_control_character_in_a_literal_multiline_string_is_not_echoed() {
    assert_secret_not_echoed("token = '''\nabcDEF123SECRET==\u{1}\n'''\n");
}

#[test]
fn ml_eq_badline_an_equals_sign_inside_a_multiline_string_is_not_a_key() {
    assert_secret_not_echoed("token = \"\"\"\nSECRETPART=rest\u{7f}\n\"\"\"\n");
}

#[test]
fn dup_after_ml_header_a_table_header_inside_a_multiline_string_is_not_a_table() {
    assert_secret_not_echoed("token = \"\"\"\n[SECRETHDR]\n\"\"\"\ntoken = \"x\"\n");
}

#[test]
fn dup_after_ml_arr_an_array_header_inside_a_multiline_string_is_not_a_table() {
    assert_secret_not_echoed("token = '''\n[[SECRET_ARR]]\n'''\ntoken = \"x\"\n");
}

#[derive(serde::Deserialize, Debug)]
#[allow(dead_code)]
#[serde(deny_unknown_fields)]
struct Strict {
    bind: Option<String>,
}

#[test]
fn an_unknown_key_is_never_named_since_it_may_be_a_secret() {
    // A token pasted on a line of its own parses as a key, and a strict table refuses it.
    let error =
        parse_toml::<Strict>("SECRETKEY = 1\n", Path::new("daemon_config.toml")).unwrap_err();
    assert_eq!(error.kind, ParseErrorKind::UnknownField);
    assert_hidden(&error, "SECRETKEY");
}
