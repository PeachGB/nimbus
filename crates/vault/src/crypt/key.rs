//! [`CryptKey`]: the 32-byte master key, its text form, and the keys derived from it.

use base64::Engine;
use ring::{aead, hkdf, rand::SecureRandom};
use zeroize::Zeroizing;

use super::{
    CryptError,
    format::{HEADER_LEN, KEY_ID_LEN},
};

/// Prefix of a key's text form; the version in it is the key format's, not the object format's.
const TEXT_PREFIX: &str = "nimbus-crypt-key-1:";
/// Length of the base64 part of the text form: 32 bytes, unpadded.
const TEXT_KEY_LEN: usize = 43;
/// HKDF info for the key id.
const KEY_ID_INFO: &[u8] = b"nimbus-crypt v1 key id";
/// HKDF info prefix for an object's payload key; the object's header follows it.
const PAYLOAD_INFO: &[u8] = b"nimbus-crypt v1 payload";

const BASE64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// A nimbus-crypt master key: 32 secret bytes that every object's key is derived from. Losing
/// it loses the data.
///
/// The bytes live in a [`Zeroizing`] buffer, wiped when the key is dropped, and so does the
/// text form [`CryptKey::to_text`] returns. `Debug` never shows them, and there is deliberately
/// no `==`. Keys derived from it stay inside `ring`'s types (the payload key goes from HKDF
/// straight into an AEAD key and never exists as plain bytes here), but note that `ring` does
/// not zeroize its own key structs when they are dropped.
///
/// # Examples
///
/// ```
/// use nimbus_vault::crypt::CryptKey;
///
/// let key = CryptKey::generate()?;
/// let text = key.to_text(); // what a key file holds
/// assert!(text.starts_with("nimbus-crypt-key-1:"));
/// let again = CryptKey::parse(&text)?;
/// assert_eq!(format!("{again:?}"), "CryptKey(<redacted>)");
/// # Ok::<(), nimbus_vault::crypt::CryptError>(())
/// ```
#[derive(Clone)]
pub struct CryptKey {
    bytes: Zeroizing<[u8; 32]>,
}

impl CryptKey {
    /// A new random key from the OS random number generator.
    pub fn generate() -> Result<Self, CryptError> {
        let mut bytes = Zeroizing::new([0u8; 32]);
        fill_random(bytes.as_mut())?;
        Ok(CryptKey { bytes })
    }

    /// Reads a key from its text form, `nimbus-crypt-key-1:` and 43 characters of unpadded
    /// URL-safe base64. Surrounding whitespace is not accepted: read a key file with
    /// [`crate::fsutil::read_secret`], which drops the trailing newline, and keep the text in a
    /// [`Zeroizing`] string. The error never quotes the text.
    pub fn parse(text: &str) -> Result<Self, CryptError> {
        let encoded = text.strip_prefix(TEXT_PREFIX).ok_or(CryptError::BadKey(
            "it doesn't start with nimbus-crypt-key-1:",
        ))?;
        if encoded.len() != TEXT_KEY_LEN {
            return Err(CryptError::BadKey("it isn't 32 bytes of base64"));
        }
        // Decoded straight into a wiped buffer; one spare byte, as the decoder wants room.
        let mut decoded = Zeroizing::new([0u8; 33]);
        let len = BASE64
            .decode_slice(encoded, decoded.as_mut())
            .map_err(|_| CryptError::BadKey("it isn't valid base64"))?;
        if len != 32 {
            return Err(CryptError::BadKey("it isn't 32 bytes of base64"));
        }
        let mut bytes = Zeroizing::new([0u8; 32]);
        bytes.copy_from_slice(&decoded[..32]);
        Ok(CryptKey { bytes })
    }

    /// The key's text form, for a key file, in a string wiped when dropped. Its capacity is
    /// reserved up front, so it is never reallocated (which would leave a copy behind).
    pub fn to_text(&self) -> Zeroizing<String> {
        let mut text = Zeroizing::new(String::with_capacity(TEXT_PREFIX.len() + TEXT_KEY_LEN));
        text.push_str(TEXT_PREFIX);
        BASE64.encode_string(self.bytes.as_ref(), &mut text);
        text
    }

    /// The key from raw bytes, for the frozen test vectors.
    #[cfg(test)]
    pub(crate) fn from_bytes(bytes: [u8; 32]) -> Self {
        CryptKey {
            bytes: Zeroizing::new(bytes),
        }
    }

    /// The raw bytes, for checking against an independent implementation in tests.
    #[cfg(test)]
    pub(crate) fn bytes_for_test(&self) -> [u8; 32] {
        *self.bytes
    }

    /// The 8-byte key id written into every header: a hint that lets a reader report the wrong
    /// key rather than damage. It identifies the key, not its value.
    pub(crate) fn key_id(&self) -> [u8; KEY_ID_LEN] {
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, &[]).extract(self.bytes.as_ref());
        let mut id = [0u8; KEY_ID_LEN];
        prk.expand(&[KEY_ID_INFO], Len(KEY_ID_LEN))
            .and_then(|okm| okm.fill(&mut id))
            .expect("HKDF-SHA256 can produce 8 bytes");
        id
    }

    /// The ChaCha20-Poly1305 key for the object whose header is `header`: HKDF-SHA256 with the
    /// header's salt, over the master key, with info `nimbus-crypt v1 payload` followed by the
    /// whole header, so any change to the header changes the key.
    pub(crate) fn payload_key(&self, header: &[u8; HEADER_LEN]) -> aead::LessSafeKey {
        let salt = &header[HEADER_LEN - super::format::SALT_LEN..];
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(self.bytes.as_ref());
        let info = [PAYLOAD_INFO, header.as_slice()];
        let okm = prk
            .expand(&info, &aead::CHACHA20_POLY1305)
            .expect("HKDF-SHA256 can produce a 32-byte key");
        aead::LessSafeKey::new(aead::UnboundKey::from(okm))
    }
}

impl std::fmt::Debug for CryptKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CryptKey(<redacted>)")
    }
}

/// Fills `buf` from the OS random number generator.
pub(crate) fn fill_random(buf: &mut [u8]) -> Result<(), CryptError> {
    ring::rand::SystemRandom::new()
        .fill(buf)
        .map_err(|_| CryptError::Rng)
}

/// An HKDF output length, for derived values that aren't keys.
struct Len(usize);

impl hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

#[cfg(test)]
#[path = "tests/key.rs"]
mod tests;
