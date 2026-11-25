use super::*;
use axum::http::HeaderValue;

fn headers(authorization: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(authorization).unwrap(),
    );
    headers
}

#[test]
fn no_auth_configured_lets_everything_through_as_anonymous() {
    let identity = Auth::None.authenticate(&HeaderMap::new()).unwrap();
    assert_eq!(identity, Identity::anonymous());
}

#[test]
fn bearer_accepts_the_configured_token() {
    let auth = Auth::Bearer {
        token: "s3cr3t".into(),
    };
    assert!(auth.authenticate(&headers("Bearer s3cr3t")).is_ok());
}

#[test]
fn the_bearer_scheme_is_matched_case_insensitively() {
    let auth = Auth::Bearer {
        token: "s3cr3t".into(),
    };
    assert!(auth.authenticate(&headers("bearer s3cr3t")).is_ok());
    assert!(auth.authenticate(&headers("BEARER s3cr3t")).is_ok());
}

#[test]
fn bearer_rejects_a_wrong_token() {
    let auth = Auth::Bearer {
        token: "s3cr3t".into(),
    };
    assert_eq!(
        auth.authenticate(&headers("Bearer nope")),
        Err(AuthError::Invalid)
    );
}

#[test]
fn bearer_rejects_a_prefix_of_the_token() {
    let auth = Auth::Bearer {
        token: "s3cr3t".into(),
    };
    assert_eq!(
        auth.authenticate(&headers("Bearer s3cr")),
        Err(AuthError::Invalid)
    );
}

#[test]
fn bearer_rejects_a_missing_header() {
    let auth = Auth::Bearer {
        token: "s3cr3t".into(),
    };
    assert_eq!(
        auth.authenticate(&HeaderMap::new()),
        Err(AuthError::Missing)
    );
}

#[test]
fn bearer_rejects_another_scheme() {
    let auth = Auth::Bearer {
        token: "s3cr3t".into(),
    };
    assert_eq!(
        auth.authenticate(&headers("Basic s3cr3t")),
        Err(AuthError::Missing)
    );
}

#[test]
fn only_the_none_scheme_counts_as_open() {
    assert!(AuthConfig::None.is_open());
    assert!(
        !AuthConfig::Bearer {
            token: Some("s3cr3t".into()),
            token_file: None,
        }
        .is_open()
    );
}

// --- Resolving the [auth] table ---

fn bearer_config(token: Option<&str>, token_file: Option<PathBuf>) -> AuthConfig {
    AuthConfig::Bearer {
        token: token.map(Secret::from),
        token_file,
    }
}

/// A `token_file` config pointing at a file in `dir` that holds `contents`.
fn token_file_config(dir: &Path, contents: &str) -> AuthConfig {
    let path = dir.join("token");
    std::fs::write(&path, contents).unwrap();
    bearer_config(None, Some(path))
}

/// The error `config.resolve()` gives, with its context chain.
fn resolve_error(config: &AuthConfig) -> String {
    match config.resolve() {
        Ok(auth) => panic!("{config:?} should have been refused, got {auth:?}"),
        Err(e) => format!("{e:#}"),
    }
}

fn token_of(auth: Auth) -> String {
    match auth {
        Auth::Bearer { token } => token.expose().to_string(),
        other => panic!("expected a bearer token, got {other:?}"),
    }
}

#[test]
fn resolve_reads_the_token_from_token_file() {
    let dir = tempfile::tempdir().unwrap();
    let config = token_file_config(dir.path(), "s3cr3t");
    assert_eq!(token_of(config.resolve().unwrap()), "s3cr3t");
}

#[test]
fn resolve_drops_the_newline_a_token_file_ends_with() {
    let dir = tempfile::tempdir().unwrap();
    let config = token_file_config(dir.path(), "s3cr3t\n");
    assert_eq!(token_of(config.resolve().unwrap()), "s3cr3t");
}

#[test]
fn resolve_refuses_both_token_and_token_file() {
    let config = bearer_config(Some("s3cr3t"), Some(PathBuf::from("/etc/nimbus/token")));
    let error = resolve_error(&config);
    assert!(
        error.contains("set only one of token or token_file"),
        "{error}"
    );
}

#[test]
fn resolve_refuses_a_bearer_table_with_neither() {
    let error = resolve_error(&bearer_config(None, None));
    assert!(error.contains("set one of token or token_file"), "{error}");
}

#[test]
fn resolve_names_a_missing_token_file() {
    let error = resolve_error(&bearer_config(
        None,
        Some(PathBuf::from("/nonexistent/token")),
    ));
    assert!(error.contains("/nonexistent/token"), "{error}");
}

#[test]
fn resolve_refuses_an_empty_token_file() {
    let dir = tempfile::tempdir().unwrap();
    let error = resolve_error(&token_file_config(dir.path(), "\n"));
    let path = dir.path().join("token").display().to_string();
    assert!(error.contains(&path), "{error}");
    assert!(error.contains("empty"), "{error}");
}

#[test]
fn resolve_refuses_an_empty_inline_token() {
    let error = resolve_error(&bearer_config(Some(""), None));
    assert!(error.contains("the configured token is empty"), "{error}");
}

#[test]
fn a_token_read_from_a_file_authenticates_requests() {
    let dir = tempfile::tempdir().unwrap();
    let auth = token_file_config(dir.path(), "s3cr3t\n").resolve().unwrap();
    assert!(auth.authenticate(&headers("Bearer s3cr3t")).is_ok());
    assert_eq!(
        auth.authenticate(&headers("Bearer nope")),
        Err(AuthError::Invalid)
    );
}

#[test]
fn debug_output_never_shows_the_token() {
    let auth = Auth::Bearer {
        token: "s3cr3t".into(),
    };
    let debug = format!("{auth:?}");
    assert!(!debug.contains("s3cr3t"), "{debug}");
    assert!(debug.contains("redacted"), "{debug}");
}
