use super::*;

#[test]
fn a_key_round_trips_through_its_text_form() {
    let key = CryptKey::generate().unwrap();
    let text = key.to_text();
    assert!(text.starts_with("nimbus-crypt-key-1:"));
    assert_eq!(text.len(), "nimbus-crypt-key-1:".len() + 43);
    let again = CryptKey::parse(&text).unwrap();
    assert_eq!(*again.bytes, *key.bytes);
}

#[test]
fn generated_keys_differ() {
    let a = CryptKey::generate().unwrap();
    let b = CryptKey::generate().unwrap();
    assert_ne!(*a.bytes, *b.bytes);
}

#[test]
fn a_malformed_key_text_is_refused_without_quoting_it() {
    let good = CryptKey::generate().unwrap().to_text();
    let body = &good["nimbus-crypt-key-1:".len()..];
    let cases = [
        body.to_string(),                              // no prefix
        format!("nimbus-crypt-key-2:{body}"),          // another key version
        format!("nimbus-crypt-key-1:{}", &body[1..]),  // too short
        format!("nimbus-crypt-key-1:{body}A"),         // too long
        format!("nimbus-crypt-key-1:{}!", &body[1..]), // not base64
        format!("nimbus-crypt-key-1:{}=", &body[1..]), // padding isn't the format
        format!(" {}", good.as_str()),                 // surrounding whitespace
        format!("nimbus-crypt-key-1:{}/", &body[1..]), // standard, not URL-safe, alphabet
    ];
    for text in cases {
        let error = CryptKey::parse(&text).unwrap_err();
        assert!(
            matches!(error, CryptError::BadKey(_)),
            "{text:?}: {error:?}"
        );
        for shown in [error.to_string(), format!("{error:?}")] {
            assert!(!shown.contains(&body[2..20]), "{shown}");
        }
    }
}

#[test]
fn a_key_never_shows_in_debug() {
    let key = CryptKey::generate().unwrap();
    let text = key.to_text();
    let body = &text["nimbus-crypt-key-1:".len()..];
    let debug = format!("{key:?} {key:#?}");
    assert_eq!(format!("{key:?}"), "CryptKey(<redacted>)");
    assert!(!debug.contains(body), "{debug}");
}

#[test]
fn the_key_id_is_stable_per_key_and_differs_between_keys() {
    let key = CryptKey::from_bytes([1; 32]);
    assert_eq!(key.key_id(), CryptKey::from_bytes([1; 32]).key_id());
    assert_ne!(key.key_id(), CryptKey::from_bytes([2; 32]).key_id());
    // Not a prefix of the key: it's derived, not copied.
    assert_ne!(key.key_id(), [1; 8]);
}
