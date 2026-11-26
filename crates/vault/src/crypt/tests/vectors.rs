//! Frozen test vectors for the v1 format, and an independent implementation of the spec
//! (RustCrypto's ChaCha20-Poly1305 and HKDF, dev-dependencies only) that checks them both ways.
//! If one of these fails, the format changed: that breaks every object already written, so it
//! needs a new version, not a new vector.

use super::*;
use futures::StreamExt;
use sha2::Digest;

/// Key bytes 0, 1, …, 31.
fn golden_key() -> CryptKey {
    CryptKey::from_bytes(GOLDEN_KEY)
}

const GOLDEN_KEY: [u8; 32] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31,
];
/// Salt bytes 32, 33, …, 63.
const GOLDEN_SALT: [u8; 32] = [
    32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55,
    56, 57, 58, 59, 60, 61, 62, 63,
];
const GOLDEN_KEY_TEXT: &str = "nimbus-crypt-key-1:AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";
/// An empty plaintext under the golden key and salt: header and footer.
const GOLDEN_EMPTY: &str = "4e4d424301000000ed4cb59dc1054e9c202122232425262728292a2b2c2d2e2f\
                            303132333435363738393a3b3c3d3e3f59e5790d8cc95b27b69f82e8a5681806\
                            0bdf8d14562bd897";
/// `hello, nimbus` under the golden key and salt.
const GOLDEN_HELLO: &str = "4e4d424301000000ed4cb59dc1054e9c202122232425262728292a2b2c2d2e2f\
                            303132333435363738393a3b3c3d3e3f4de66bb79518ce5c74ce9d34cf46d647\
                            72afdfec2dc6454d1ef7888d1802c41261d577015bf6c20a0eee08c63f12dfb3\
                            0249c2be4b";
/// SHA-256 of [`big_plaintext`] under the golden key and salt (65641 bytes: two data chunks).
const GOLDEN_BIG_SHA256: &str = "2c33a4b1fe8340c34f01fc917fd87f1e8ec9197e187642a50adbd619c8dca067";

/// 65537 bytes: one full chunk and a one-byte chunk.
fn big_plaintext() -> Vec<u8> {
    (0..65537u32).map(|i| (i % 251) as u8).collect()
}

/// `plaintext` sealed under the golden key and salt.
fn golden_seal(plaintext: &[u8]) -> Vec<u8> {
    golden_seal_with(&golden_key(), plaintext)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Vec<u8> {
    let text: String = text.split_whitespace().collect();
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

// --- frozen bytes ---

#[test]
fn the_golden_key_text_is_frozen() {
    assert_eq!(golden_key().to_text().as_str(), GOLDEN_KEY_TEXT);
    let parsed = CryptKey::parse(GOLDEN_KEY_TEXT).unwrap();
    assert_eq!(
        hex(&golden_seal_with(&parsed, b"")),
        hex(&unhex(GOLDEN_EMPTY))
    );
}

/// `plaintext` sealed under `key` and the golden salt.
fn golden_seal_with(key: &CryptKey, plaintext: &[u8]) -> Vec<u8> {
    let input = futures::stream::iter([Ok(bytes::Bytes::copy_from_slice(plaintext))]).boxed();
    let sealed = stream::encrypt_with_salt(key, GOLDEN_SALT, input);
    futures::executor::block_on(async {
        let parts: Vec<bytes::Bytes> = futures::TryStreamExt::try_collect(sealed).await.unwrap();
        parts.concat()
    })
}

#[test]
fn the_golden_ciphertexts_are_frozen() {
    assert_eq!(hex(&golden_seal(b"")), hex(&unhex(GOLDEN_EMPTY)));
    assert_eq!(
        hex(&golden_seal(b"hello, nimbus")),
        hex(&unhex(GOLDEN_HELLO))
    );
    let big = golden_seal(&big_plaintext());
    assert_eq!(big.len(), 65641);
    assert_eq!(hex(&sha2::Sha256::digest(&big)), GOLDEN_BIG_SHA256);
}

#[test]
fn the_golden_ciphertexts_decrypt() {
    let key = golden_key();
    assert_eq!(decrypt_bytes(&key, &unhex(GOLDEN_EMPTY)).unwrap(), b"");
    assert_eq!(
        decrypt_bytes(&key, &unhex(GOLDEN_HELLO)).unwrap(),
        b"hello, nimbus"
    );
}

// --- an independent implementation of the spec, from the module docs alone ---

mod independent {
    use chacha20poly1305::{ChaCha20Poly1305, KeyInit, aead::Aead};
    use hkdf::Hkdf;
    use sha2::Sha256;

    const CHUNK: usize = 65536;
    const TAG: usize = 16;

    fn nonce(index: u32, last: bool) -> [u8; 12] {
        let mut nonce = [0u8; 12];
        nonce[7..11].copy_from_slice(&index.to_be_bytes());
        nonce[11] = u8::from(last);
        nonce
    }

    pub fn key_id(master: &[u8; 32]) -> [u8; 8] {
        let mut id = [0u8; 8];
        Hkdf::<Sha256>::new(Some(&[]), master)
            .expand(b"nimbus-crypt v1 key id", &mut id)
            .unwrap();
        id
    }

    fn payload_cipher(master: &[u8; 32], header: &[u8]) -> ChaCha20Poly1305 {
        let mut info = b"nimbus-crypt v1 payload".to_vec();
        info.extend_from_slice(header);
        let mut key = [0u8; 32];
        Hkdf::<Sha256>::new(Some(&header[16..48]), master)
            .expand(&info, &mut key)
            .unwrap();
        ChaCha20Poly1305::new(&key.into())
    }

    pub fn encrypt(master: &[u8; 32], salt: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
        let mut header = b"NMBC\x01\x00\x00\x00".to_vec();
        header.extend_from_slice(&key_id(master));
        header.extend_from_slice(salt);
        let cipher = payload_cipher(master, &header);
        let mut out = header;
        let mut index = 0u32;
        for chunk in plaintext.chunks(CHUNK) {
            out.extend(cipher.encrypt(&nonce(index, false).into(), chunk).unwrap());
            index += 1;
        }
        let footer = (plaintext.len() as u64).to_be_bytes();
        out.extend(
            cipher
                .encrypt(&nonce(index, true).into(), &footer[..])
                .unwrap(),
        );
        out
    }

    pub fn decrypt(master: &[u8; 32], object: &[u8]) -> Vec<u8> {
        let (header, body) = object.split_at(48);
        assert_eq!(&header[..8], b"NMBC\x01\x00\x00\x00");
        assert_eq!(header[8..16], key_id(master));
        let cipher = payload_cipher(master, header);
        let (data, footer) = body.split_at(body.len() - (8 + TAG));
        let mut out = Vec::new();
        let mut index = 0u32;
        for sealed in data.chunks(CHUNK + TAG) {
            out.extend(cipher.decrypt(&nonce(index, false).into(), sealed).unwrap());
            index += 1;
        }
        let length = cipher.decrypt(&nonce(index, true).into(), footer).unwrap();
        assert_eq!(length, (out.len() as u64).to_be_bytes());
        out
    }
}

#[test]
fn an_independent_implementation_reads_what_ring_wrote() {
    assert_eq!(independent::decrypt(&GOLDEN_KEY, &unhex(GOLDEN_EMPTY)), b"");
    assert_eq!(
        independent::decrypt(&GOLDEN_KEY, &unhex(GOLDEN_HELLO)),
        b"hello, nimbus"
    );
    assert_eq!(
        independent::decrypt(&GOLDEN_KEY, &golden_seal(&big_plaintext())),
        big_plaintext()
    );
    // And a random key and salt, so the vectors aren't the only inputs checked.
    let key = CryptKey::generate().unwrap();
    let sealed = encrypt_bytes(&key, &big_plaintext()).unwrap();
    assert_eq!(
        independent::decrypt(&key.bytes_for_test(), &sealed),
        big_plaintext()
    );
}

#[test]
fn an_independent_implementation_writes_the_same_bytes() {
    for plaintext in [b"".to_vec(), b"hello, nimbus".to_vec(), big_plaintext()] {
        let theirs = independent::encrypt(&GOLDEN_KEY, &GOLDEN_SALT, &plaintext);
        assert_eq!(hex(&theirs), hex(&golden_seal(&plaintext)));
        assert_eq!(decrypt_bytes(&golden_key(), &theirs).unwrap(), plaintext);
    }
    assert_eq!(independent::key_id(&GOLDEN_KEY), golden_key().key_id());
}
