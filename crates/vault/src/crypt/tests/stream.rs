use super::*;
use crate::crypt::{ciphertext_len, format::Header};
use futures::{TryStreamExt, stream};

fn key() -> CryptKey {
    CryptKey::from_bytes([0x42; 32])
}

/// `len` bytes that differ from chunk to chunk, so a swapped chunk can't decrypt "correctly".
fn pattern(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i % 251) as u8 ^ (i / 65536) as u8)
        .collect()
}

/// `bytes` as a stream of `piece`-sized items, to exercise every way a chunk can be split.
fn pieces(bytes: &[u8], piece: usize) -> ByteStream {
    let items: Vec<Result<Bytes, VaultError>> = bytes
        .chunks(piece.max(1))
        .map(|c| Ok(Bytes::copy_from_slice(c)))
        .collect();
    stream::iter(items).boxed()
}

fn block_on_collect(stream: ByteStream) -> Result<Vec<u8>, VaultError> {
    futures::executor::block_on(async {
        let parts: Vec<Bytes> = stream.try_collect().await?;
        Ok(parts.concat())
    })
}

fn open(ciphertext: &[u8]) -> Result<Vec<u8>, CryptError> {
    decrypt_bytes(&key(), ciphertext)
}

fn seal(plaintext: &[u8]) -> Vec<u8> {
    encrypt_bytes(&key(), plaintext).unwrap()
}

/// Byte range of chunk `index` (0-based, data chunks then the footer) in an object.
fn chunk_range(ciphertext_len: usize, index: usize) -> std::ops::Range<usize> {
    let start = HEADER_LEN + index * SEALED_CHUNK_LEN;
    let end = (start + SEALED_CHUNK_LEN).min(ciphertext_len - SEALED_FOOTER_LEN);
    if start >= end {
        // The footer.
        ciphertext_len - SEALED_FOOTER_LEN..ciphertext_len
    } else {
        start..end
    }
}

// --- round trips and sizes ---

#[test]
fn every_size_round_trips_however_the_streams_are_split() {
    for len in [0, 1, 65535, 65536, 65537, 131072, 131073, 200_000] {
        let plaintext = pattern(len);
        for piece in [1000, 65536, 70_001, len.max(1)] {
            let sealed =
                block_on_collect(encrypt(&key(), pieces(&plaintext, piece)).unwrap()).unwrap();
            assert_eq!(
                sealed.len() as u64,
                ciphertext_len(len as u64).unwrap(),
                "P = {len}"
            );
            for read_piece in [7, 65552, 100_000] {
                let opened = block_on_collect(decrypt(&key(), pieces(&sealed, read_piece)));
                assert_eq!(
                    opened.unwrap(),
                    plaintext,
                    "P = {len}, pieces {piece}/{read_piece}"
                );
            }
        }
    }
}

#[test]
fn a_tiny_object_decrypts_from_one_byte_pieces() {
    let sealed = seal(b"hello");
    let opened = block_on_collect(decrypt(&key(), pieces(&sealed, 1))).unwrap();
    assert_eq!(opened, b"hello");
}

#[test]
fn an_empty_plaintext_still_has_a_footer() {
    let sealed = seal(b"");
    assert_eq!(sealed.len(), HEADER_LEN + SEALED_FOOTER_LEN);
    assert_eq!(open(&sealed).unwrap(), b"");
}

#[test]
fn every_encryption_draws_a_fresh_salt() {
    let a = seal(b"same plaintext");
    let b = seal(b"same plaintext");
    assert_ne!(a[16..48], b[16..48], "salt reused");
    assert_ne!(a[48..], b[48..], "ciphertext repeated");
}

// --- wrong input ---

#[test]
fn a_wrong_key_is_a_wrong_key_not_damage() {
    let sealed = seal(&pattern(70_000));
    let other = CryptKey::from_bytes([0x43; 32]);
    assert_eq!(decrypt_bytes(&other, &sealed), Err(CryptError::WrongKey));
}

#[test]
fn an_empty_object_is_truncated() {
    assert_eq!(open(b""), Err(CryptError::Truncated));
}

#[test]
fn a_plaintext_object_is_not_encrypted() {
    assert_eq!(open(b"hello, world"), Err(CryptError::NotEncrypted));
    assert_eq!(open(&[0u8; 200]), Err(CryptError::NotEncrypted));
    // Said as soon as the magic differs, without waiting for a whole header.
    assert_eq!(open(b"N"), Err(CryptError::Truncated));
    assert_eq!(open(b"NX"), Err(CryptError::NotEncrypted));
}

#[test]
fn an_object_cut_inside_the_header_is_truncated() {
    let sealed = seal(b"hello");
    for len in [1, 4, 20, 47, 48, 50, 71] {
        assert_eq!(
            open(&sealed[..len]),
            Err(CryptError::Truncated),
            "cut at {len}"
        );
    }
}

#[test]
fn the_decoder_and_plaintext_len_agree_on_impossible_lengths() {
    // Every length plaintext_len calls impossible must decrypt as Truncated too, so a listing
    // and a read never disagree about what is wrong with an object.
    let sealed = seal(&pattern(70_000));
    for len in HEADER_LEN..sealed.len() {
        if crate::crypt::plaintext_len(len as u64) == Err(CryptError::Truncated) {
            assert_eq!(
                open(&sealed[..len]),
                Err(CryptError::Truncated),
                "cut at {len}"
            );
        }
    }
}

// --- the header is authenticated ---

#[test]
fn every_header_byte_is_checked() {
    let sealed = seal(&pattern(1000));
    for offset in 0..HEADER_LEN {
        let mut bad = sealed.clone();
        bad[offset] ^= 0x01;
        let error = open(&bad).unwrap_err();
        let expected = match offset {
            0..4 => CryptError::NotEncrypted,
            4 => CryptError::UnsupportedVersion(0),
            5..8 => CryptError::UnsupportedHeader,
            // The key id only chooses the message; the AEAD failure is what refuses it.
            8..16 => CryptError::WrongKey,
            // The salt feeds HKDF, so the payload key changes and chunk 0 fails.
            _ => CryptError::Corrupt { chunk: 0 },
        };
        assert_eq!(error, expected, "header byte {offset}");
    }
}

#[test]
fn a_modified_key_id_with_the_right_key_still_fails() {
    // The key id is in the HKDF info, so even with the right key a changed id changes the
    // payload key: the object must not decrypt.
    let mut sealed = seal(b"hello");
    sealed[10] ^= 0xff;
    assert!(open(&sealed).is_err());
}

// --- chunks ---

#[test]
fn a_flipped_byte_anywhere_in_a_chunk_is_caught() {
    let sealed = seal(&pattern(140_000)); // three data chunks and a footer
    for (chunk, offset) in [(0, 0), (0, 65551), (1, 30_000), (2, 5), (3, 3), (3, 23)] {
        let range = chunk_range(sealed.len(), chunk);
        let mut bad = sealed.clone();
        bad[range.start + offset] ^= 0x80;
        assert_eq!(
            open(&bad),
            Err(CryptError::Corrupt {
                chunk: chunk as u64
            }),
            "chunk {chunk} offset {offset}"
        );
    }
}

#[test]
fn reordered_dropped_or_duplicated_chunks_are_caught() {
    let sealed = seal(&pattern(3 * 65536)); // three full data chunks and a footer
    let chunk = |i| sealed[chunk_range(sealed.len(), i)].to_vec();
    let header = sealed[..HEADER_LEN].to_vec();
    let assemble = |order: &[usize]| {
        let mut out = header.clone();
        for &i in order {
            out.extend(chunk(i));
        }
        out
    };
    assert!(open(&assemble(&[0, 1, 2, 3])).is_ok());
    for order in [
        vec![1, 0, 2, 3],    // swapped
        vec![0, 2, 3],       // a data chunk dropped
        vec![0, 1, 1, 2, 3], // duplicated
        vec![0, 1, 2],       // the footer dropped, cut exactly at a chunk boundary
        vec![0, 1, 3],       // the last data chunk dropped
    ] {
        assert!(
            matches!(open(&assemble(&order)), Err(CryptError::Corrupt { .. })),
            "{order:?}"
        );
    }
}

#[test]
fn a_chunk_from_another_object_with_the_same_key_is_caught() {
    let a = seal(&pattern(2 * 65536));
    let b = seal(&pattern(2 * 65536));
    let mut spliced = a.clone();
    let range = chunk_range(a.len(), 1);
    spliced[range.clone()].copy_from_slice(&b[range]);
    assert_eq!(open(&spliced), Err(CryptError::Corrupt { chunk: 1 }));
}

#[test]
fn trailing_data_is_caught() {
    let sealed = seal(&pattern(1000));
    for extra in [1, 16, 17, 24, 100, SEALED_CHUNK_LEN] {
        let mut longer = sealed.clone();
        longer.extend(std::iter::repeat_n(0xAA, extra));
        let error = open(&longer).unwrap_err();
        assert!(
            matches!(error, CryptError::Corrupt { .. } | CryptError::Truncated),
            "{extra} extra bytes: {error:?}"
        );
    }
}

#[test]
fn any_cut_is_caught() {
    let sealed = seal(&pattern(70_000));
    for len in (HEADER_LEN..sealed.len())
        .step_by(997)
        .chain([sealed.len() - 1])
    {
        assert!(open(&sealed[..len]).is_err(), "cut at {len}");
    }
}

// --- the last flag and the footer ---

/// The header every hand-sealed object below uses.
fn fixed_header() -> [u8; HEADER_LEN] {
    Header {
        key_id: key().key_id(),
        salt: [5; SALT_LEN],
    }
    .encode()
}

/// Chunk `index` of a [`fixed_header`] object, sealed by hand with the given last flag.
fn sealed_chunk(index: u32, plaintext: &[u8], last: bool) -> Vec<u8> {
    let mut chunk = plaintext.to_vec();
    Sealer::new(key().payload_key(&fixed_header()), index)
        .seal(&mut chunk, last)
        .unwrap();
    chunk
}

/// A [`fixed_header`] object made of `chunks`, each `(plaintext, last)`, numbered in order.
fn hand_sealed(chunks: &[(&[u8], bool)]) -> Vec<u8> {
    let mut out = fixed_header().to_vec();
    for (index, (plaintext, last)) in chunks.iter().enumerate() {
        out.extend(sealed_chunk(index as u32, plaintext, *last));
    }
    out
}

#[test]
fn the_footer_must_carry_the_last_flag() {
    let footer = 3u64.to_be_bytes();
    assert_eq!(
        open(&hand_sealed(&[(b"abc", false), (&footer, true)])).unwrap(),
        b"abc"
    );
    // The footer without the flag: an object that never ends.
    assert_eq!(
        open(&hand_sealed(&[(b"abc", false), (&footer, false)])),
        Err(CryptError::Corrupt { chunk: 1 })
    );
}

#[test]
fn only_the_footer_may_carry_the_last_flag() {
    // A data chunk flagged last, followed by a well-formed footer: the data chunk fails.
    let footer = 3u64.to_be_bytes();
    assert_eq!(
        open(&hand_sealed(&[(b"abc", true), (&footer, true)])),
        Err(CryptError::Corrupt { chunk: 0 })
    );
    // And the sealer refuses to seal anything after a last chunk.
    let mut sealer = Sealer::new(key().payload_key(&fixed_header()), 0);
    sealer.seal(&mut b"abc".to_vec(), true).unwrap();
    assert_eq!(
        sealer.seal(&mut footer.to_vec(), true),
        Err(CryptError::TooLarge)
    );
}

#[test]
fn a_footer_with_the_wrong_length_is_refused() {
    let wrong = 4u64.to_be_bytes();
    assert_eq!(
        open(&hand_sealed(&[(b"abc", false), (&wrong, true)])),
        Err(CryptError::LengthMismatch)
    );
}

#[test]
fn the_chunk_counter_never_wraps() {
    // At the last index only the footer fits: a data chunk there leaves no index for it.
    let payload_key = || key().payload_key(&fixed_header());
    let mut sealer = Sealer::new(payload_key(), u32::MAX);
    assert_eq!(
        sealer.seal(&mut b"data".to_vec(), false),
        Err(CryptError::TooLarge)
    );
    let mut sealer = Sealer::new(payload_key(), u32::MAX);
    assert_eq!(sealer.seal(&mut b"footer".to_vec(), true), Ok(()));
}

// --- the stream itself ---

#[test]
fn an_error_from_the_input_ends_the_stream() {
    let failing = || {
        stream::iter([
            Ok(Bytes::from_static(b"abc")),
            Err(VaultError::Generic("disk on fire".into())),
        ])
        .boxed()
    };
    let error = block_on_collect(encrypt(&key(), failing()).unwrap()).unwrap_err();
    assert!(error.to_string().contains("disk on fire"));
    let error = block_on_collect(decrypt(&key(), failing())).unwrap_err();
    assert!(
        matches!(error, VaultError::Generic(_) | VaultError::Crypt(_)),
        "{error:?}"
    );
}

// --- fuzzing the decoder: anything that isn't a sealed object is an error, never a panic ---

/// A small deterministic generator, so a failure reproduces.
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// One random edit of `bytes`: a flip, a cut, an insertion, a deletion or an appended run.
fn mutate(rng: &mut XorShift, bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    let at = rng.below(out.len() + 1);
    match rng.below(5) {
        0 if !out.is_empty() => {
            let i = at.min(out.len() - 1);
            out[i] ^= 1 << rng.below(8);
        }
        1 => out.truncate(at),
        2 => out.insert(at, rng.next() as u8),
        3 if at < out.len() => {
            out.remove(at);
        }
        _ => out.extend((0..1 + rng.below(40)).map(|_| rng.next() as u8)),
    }
    out
}

#[test]
fn the_decoder_rejects_every_mutation_without_panicking() {
    let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
    let originals: Vec<Vec<u8>> = [0, 1, 17, 1000, 65536, 70_000]
        .iter()
        .map(|&len| seal(&pattern(len)))
        .collect();
    for round in 0..3000 {
        let original = &originals[round % originals.len()];
        let mut bytes = mutate(&mut rng, original);
        for _ in 0..rng.below(3) {
            bytes = mutate(&mut rng, &bytes);
        }
        if &bytes == original {
            continue;
        }
        assert!(
            open(&bytes).is_err(),
            "round {round} decrypted a modified object"
        );
    }
    // Random bytes behind a valid-looking header prefix.
    for _ in 0..500 {
        let mut bytes = b"NMBC\x01\x00\x00\x00".to_vec();
        bytes.extend((0..rng.below(300)).map(|_| rng.next() as u8));
        assert!(open(&bytes).is_err());
    }
}

#[test]
fn a_footer_at_the_last_index_opens() {
    // An object with 2^32 - 1 data chunks has its footer at u32::MAX: opening it must not try
    // to move the counter past the end.
    let mut footer = 0u64.to_be_bytes().to_vec();
    Sealer::new(key().payload_key(&fixed_header()), u32::MAX)
        .seal(&mut footer, true)
        .unwrap();
    let mut opener = Opener {
        key: key().payload_key(&fixed_header()),
        next: u32::MAX,
        key_id_matches: true,
        plaintext_len: 0,
    };
    assert_eq!(opener.open(&mut footer, true), Ok(8));
    assert_eq!(opener.next, u32::MAX);
}
