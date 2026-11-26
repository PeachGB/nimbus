use super::*;

const SEALED: u64 = SEALED_CHUNK_LEN as u64;

#[test]
fn sizes_round_trip_at_every_chunk_boundary() {
    for plaintext in [
        0,
        1,
        65535,
        65536,
        65537,
        131071,
        131072,
        131073,
        10 * 65536 + 7,
    ] {
        let ciphertext = ciphertext_len(plaintext).unwrap();
        assert_eq!(plaintext_len(ciphertext), Ok(plaintext), "P = {plaintext}");
    }
}

#[test]
fn ciphertext_len_matches_the_spec_formula() {
    assert_eq!(ciphertext_len(0), Some(72));
    assert_eq!(ciphertext_len(1), Some(72 + 1 + 16));
    assert_eq!(ciphertext_len(65535), Some(72 + 65535 + 16));
    assert_eq!(ciphertext_len(65536), Some(72 + 65536 + 16));
    assert_eq!(ciphertext_len(65537), Some(72 + 65537 + 32));
    assert_eq!(ciphertext_len(131072), Some(72 + 131072 + 32));
}

#[test]
fn impossible_lengths_are_truncated_never_an_underflow() {
    for ciphertext in [0, 1, 47, 48, 71] {
        assert_eq!(
            plaintext_len(ciphertext),
            Err(CryptError::Truncated),
            "C = {ciphertext}"
        );
    }
    // A last chunk shorter than its own tag, after zero or one full chunks.
    for body in (1..=16).chain((1..=16).map(|r| SEALED + r)) {
        assert_eq!(
            plaintext_len(72 + body),
            Err(CryptError::Truncated),
            "body = {body}"
        );
    }
    // One past a tag is a one-byte chunk: valid.
    assert_eq!(plaintext_len(72 + 17), Ok(1));
    assert_eq!(plaintext_len(72 + SEALED), Ok(65536));
}

#[test]
fn the_size_limit_is_the_chunk_counter() {
    let max_plaintext = u64::from(u32::MAX) * CHUNK_LEN as u64;
    assert!(ciphertext_len(max_plaintext).is_some());
    assert_eq!(ciphertext_len(max_plaintext + 1), None);
    assert_eq!(plaintext_len(u64::MAX), Err(CryptError::TooLarge));
}

/// A header with recognisable bytes in every field.
fn sample_header() -> Header {
    Header {
        key_id: [7; KEY_ID_LEN],
        salt: [9; SALT_LEN],
    }
}

#[test]
fn the_header_layout_matches_the_spec() {
    let bytes = sample_header().encode();
    assert_eq!(&bytes[..4], b"NMBC");
    assert_eq!(bytes[4], 1);
    assert_eq!(&bytes[5..8], &[0, 0, 0]);
    assert_eq!(&bytes[8..16], &[7; 8]);
    assert_eq!(&bytes[16..], &[9; 32]);
    assert_eq!(Header::decode(&bytes), Ok(sample_header()));
}

#[test]
fn a_header_v1_does_not_define_is_refused() {
    let good = sample_header().encode();
    let with = |offset: usize, value: u8| {
        let mut bytes = good;
        bytes[offset] = value;
        Header::decode(&bytes)
    };
    assert_eq!(with(0, b'X'), Err(CryptError::NotEncrypted));
    assert_eq!(with(4, 2), Err(CryptError::UnsupportedVersion(2)));
    assert_eq!(with(4, 0), Err(CryptError::UnsupportedVersion(0)));
    // Every flag bit, bit 0 (the future id-bound mode) included, and both reserved bytes.
    for bit in 0..8 {
        assert_eq!(
            with(5, 1 << bit),
            Err(CryptError::UnsupportedHeader),
            "bit {bit}"
        );
    }
    assert_eq!(with(6, 1), Err(CryptError::UnsupportedHeader));
    assert_eq!(with(7, 0x80), Err(CryptError::UnsupportedHeader));
}

#[test]
fn the_nonce_is_seven_zero_bytes_the_index_and_the_last_flag() {
    assert_eq!(nonce(0, false), [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(nonce(1, true), [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1]);
    assert_eq!(
        nonce(0x0102_0304, false),
        [0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 0]
    );
}

#[test]
fn the_magic_check_works_on_a_partial_prefix() {
    assert!(could_be_magic(b""));
    assert!(could_be_magic(b"NM"));
    assert!(could_be_magic(b"NMBC and more"));
    assert!(!could_be_magic(b"NX"));
    assert!(!could_be_magic(b"hello"));
}
