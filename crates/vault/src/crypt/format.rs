//! The v1 byte layout: header, chunk sizes, nonces, and the plaintext/ciphertext size relation.
//! See the [format specification](crate::crypt#format-specification-v1).

use super::CryptError;

/// The first four bytes of every nimbus-crypt object.
pub(crate) const MAGIC: [u8; 4] = *b"NMBC";
/// The only format version this build writes and reads.
pub(crate) const VERSION: u8 = 1;
/// Header length in bytes.
pub(crate) const HEADER_LEN: usize = 48;
/// Length of the random salt at the end of the header.
pub(crate) const SALT_LEN: usize = 32;
/// Length of the key id in the header.
pub(crate) const KEY_ID_LEN: usize = 8;
/// Plaintext bytes per full data chunk.
pub(crate) const CHUNK_LEN: usize = 64 * 1024;
/// Authentication tag appended to every chunk.
pub(crate) const TAG_LEN: usize = 16;
/// A full data chunk as stored.
pub(crate) const SEALED_CHUNK_LEN: usize = CHUNK_LEN + TAG_LEN;
/// The footer's plaintext: the total plaintext length, big-endian.
pub(crate) const FOOTER_LEN: usize = 8;
/// The footer as stored.
pub(crate) const SEALED_FOOTER_LEN: usize = FOOTER_LEN + TAG_LEN;
/// The smallest possible object: an empty plaintext, so a header and a footer.
pub(crate) const MIN_LEN: usize = HEADER_LEN + SEALED_FOOTER_LEN;

/// The parsed fields of a v1 header. Flags and reserved bytes are always zero in v1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Header {
    pub key_id: [u8; KEY_ID_LEN],
    pub salt: [u8; SALT_LEN],
}

impl Header {
    /// The 48 header bytes as written.
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[..4].copy_from_slice(&MAGIC);
        out[4] = VERSION;
        // out[5] (flags) and out[6..8] (reserved) stay zero.
        out[8..16].copy_from_slice(&self.key_id);
        out[16..].copy_from_slice(&self.salt);
        out
    }

    /// Parses a header, refusing anything v1 doesn't define: another magic, another version,
    /// any flag, any non-zero reserved byte.
    pub fn decode(bytes: &[u8; HEADER_LEN]) -> Result<Self, CryptError> {
        if bytes[..4] != MAGIC {
            return Err(CryptError::NotEncrypted);
        }
        if bytes[4] != VERSION {
            return Err(CryptError::UnsupportedVersion(bytes[4]));
        }
        if bytes[5..8] != [0, 0, 0] {
            return Err(CryptError::UnsupportedHeader);
        }
        let mut key_id = [0u8; KEY_ID_LEN];
        key_id.copy_from_slice(&bytes[8..16]);
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&bytes[16..]);
        Ok(Header { key_id, salt })
    }
}

/// Whether `prefix`, the first bytes of an object, could still start a nimbus-crypt object.
/// Lets a reader say "not encrypted" as soon as the magic differs.
pub(crate) fn could_be_magic(prefix: &[u8]) -> bool {
    let len = prefix.len().min(MAGIC.len());
    prefix[..len] == MAGIC[..len]
}

/// The 12-byte nonce of chunk `index`: seven zero bytes, the index big-endian, the last flag.
pub(crate) fn nonce(index: u32, last: bool) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[7..11].copy_from_slice(&index.to_be_bytes());
    nonce[11] = u8::from(last);
    nonce
}

/// The stored size of an object whose plaintext is `plaintext` bytes, or `None` if the format
/// can't hold that much (more than 2^32 − 1 data chunks).
///
/// # Examples
///
/// ```
/// use nimbus_vault::crypt::ciphertext_len;
///
/// assert_eq!(ciphertext_len(0), Some(72)); // header + footer
/// assert_eq!(ciphertext_len(5), Some(72 + 5 + 16));
/// assert_eq!(ciphertext_len(65536), Some(72 + 65536 + 16));
/// ```
pub fn ciphertext_len(plaintext: u64) -> Option<u64> {
    let chunks = plaintext.div_ceil(CHUNK_LEN as u64);
    if chunks > u64::from(u32::MAX) {
        return None;
    }
    Some(MIN_LEN as u64 + plaintext + TAG_LEN as u64 * chunks)
}

/// The plaintext size of an object stored as `ciphertext` bytes, computed from the length
/// alone. An impossible length is [`CryptError::Truncated`]: the object is shorter than an
/// empty one, or ends inside a chunk's tag.
///
/// # Examples
///
/// ```
/// use nimbus_vault::crypt::{plaintext_len, CryptError};
///
/// assert_eq!(plaintext_len(72), Ok(0));
/// assert_eq!(plaintext_len(72 + 5 + 16), Ok(5));
/// assert_eq!(plaintext_len(0), Err(CryptError::Truncated));
/// assert_eq!(plaintext_len(72 + 10), Err(CryptError::Truncated)); // less than one tag
/// ```
pub fn plaintext_len(ciphertext: u64) -> Result<u64, CryptError> {
    let body = ciphertext
        .checked_sub(MIN_LEN as u64)
        .ok_or(CryptError::Truncated)?;
    let sealed = SEALED_CHUNK_LEN as u64;
    let rest = body % sealed;
    if (1..=TAG_LEN as u64).contains(&rest) {
        return Err(CryptError::Truncated);
    }
    let chunks = body.div_ceil(sealed);
    if chunks > u64::from(u32::MAX) {
        return Err(CryptError::TooLarge);
    }
    Ok(body - TAG_LEN as u64 * chunks)
}

#[cfg(test)]
#[path = "tests/format.rs"]
mod tests;
