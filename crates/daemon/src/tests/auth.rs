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
    let identity = AuthConfig::None.authenticate(&HeaderMap::new()).unwrap();
    assert_eq!(identity, Identity::anonymous());
}

#[test]
fn bearer_accepts_the_configured_token() {
    let auth = AuthConfig::Bearer {
        token: "s3cr3t".to_string(),
    };
    assert!(auth.authenticate(&headers("Bearer s3cr3t")).is_ok());
}

#[test]
fn the_bearer_scheme_is_matched_case_insensitively() {
    let auth = AuthConfig::Bearer {
        token: "s3cr3t".to_string(),
    };
    assert!(auth.authenticate(&headers("bearer s3cr3t")).is_ok());
    assert!(auth.authenticate(&headers("BEARER s3cr3t")).is_ok());
}

#[test]
fn bearer_rejects_a_wrong_token() {
    let auth = AuthConfig::Bearer {
        token: "s3cr3t".to_string(),
    };
    assert_eq!(
        auth.authenticate(&headers("Bearer nope")),
        Err(AuthError::Invalid)
    );
}

#[test]
fn bearer_rejects_a_prefix_of_the_token() {
    let auth = AuthConfig::Bearer {
        token: "s3cr3t".to_string(),
    };
    assert_eq!(
        auth.authenticate(&headers("Bearer s3cr")),
        Err(AuthError::Invalid)
    );
}

#[test]
fn bearer_rejects_a_missing_header() {
    let auth = AuthConfig::Bearer {
        token: "s3cr3t".to_string(),
    };
    assert_eq!(
        auth.authenticate(&HeaderMap::new()),
        Err(AuthError::Missing)
    );
}

#[test]
fn bearer_rejects_another_scheme() {
    let auth = AuthConfig::Bearer {
        token: "s3cr3t".to_string(),
    };
    assert_eq!(
        auth.authenticate(&headers("Basic s3cr3t")),
        Err(AuthError::Missing)
    );
}

#[test]
fn constant_time_eq_still_answers_the_question() {
    assert!(constant_time_eq(b"abc", b"abc"));
    assert!(!constant_time_eq(b"abc", b"abd"));
    assert!(!constant_time_eq(b"abc", b"ab"));
    assert!(constant_time_eq(b"", b""));
}

#[test]
fn only_the_none_scheme_counts_as_open() {
    assert!(AuthConfig::None.is_open());
    assert!(
        !AuthConfig::Bearer {
            token: "s3cr3t".to_string()
        }
        .is_open()
    );
}
