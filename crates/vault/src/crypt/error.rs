/// Why an object couldn't be encrypted or decrypted, or a key couldn't be read. No variant ever
/// carries key material or plaintext.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CryptError {
    /// The object doesn't start with the nimbus-crypt magic: it was never encrypted (a plain
    /// file in an encrypted vault's storage, say).
    #[error("not a nimbus-crypt object")]
    NotEncrypted,
    /// A nimbus-crypt object of a version this build doesn't read.
    #[error("unsupported nimbus-crypt version {0}")]
    UnsupportedVersion(u8),
    /// Header flags or reserved bytes this version doesn't define.
    #[error("unsupported nimbus-crypt header (flags or reserved bytes set)")]
    UnsupportedHeader,
    /// The object was encrypted with another key: it fails to authenticate, and its key id
    /// isn't this key's.
    #[error("the object was encrypted with a different key (or its header was modified)")]
    WrongKey,
    /// The object ends early: empty, cut inside the header, or cut so that its length is
    /// impossible.
    #[error("the object is truncated")]
    Truncated,
    /// A chunk failed to authenticate under the right key: the object was modified, reordered,
    /// cut at a chunk boundary or extended.
    #[error("the object is corrupt or was tampered with (chunk {chunk})")]
    Corrupt {
        /// 0-based index of the chunk that failed (the footer's index if it was the footer).
        chunk: u64,
    },
    /// The footer's length doesn't match the plaintext decrypted.
    #[error("the object's recorded length doesn't match its contents")]
    LengthMismatch,
    /// The plaintext needs more chunks than the format's 32-bit chunk counter allows.
    #[error("the object is too large to encrypt")]
    TooLarge,
    /// A key's text form is malformed. The reason never quotes the text.
    #[error("invalid nimbus-crypt key: {0}")]
    BadKey(&'static str),
    /// The OS random number generator failed.
    #[error("the system random number generator failed")]
    Rng,
}
