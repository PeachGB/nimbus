use super::*;

#[test]
fn secret_debug_is_redacted() {
    let debug = format!("{:?}", Secret::from("s3cr3t"));
    assert_eq!(debug, "<redacted>");
}

#[test]
fn secret_round_trips_through_serde_as_the_plain_string() {
    #[derive(Serialize, Deserialize, Debug)]
    struct Table {
        token: Secret,
    }

    let raw = toml::to_string(&Table {
        token: "s3cr3t".into(),
    })
    .unwrap();
    assert_eq!(raw.trim(), r#"token = "s3cr3t""#);

    let parsed: Table = toml::from_str(&raw).unwrap();
    assert_eq!(parsed.token.expose(), "s3cr3t");
}

#[test]
fn secret_parses_from_str() {
    let parsed: Secret = "s3cr3t".parse().unwrap();
    assert_eq!(parsed.expose(), Secret::from("s3cr3t".to_string()).expose());
    assert!("".parse::<Secret>().unwrap().is_empty());
}

#[test]
fn secret_matches_only_the_same_bytes() {
    let secret = Secret::from("abc");
    assert!(secret.matches(b"abc"));
    for wrong in [&b"abd"[..], b"ab", b"abcd", b""] {
        assert!(!secret.matches(wrong), "{wrong:?}");
    }
    assert!(Secret::from("").matches(b""));
}
