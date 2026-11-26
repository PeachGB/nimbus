//! Encryption at rest: the nimbus-crypt v1 object format, the key that opens it, and streaming
//! encryption and decryption. The `crypt` origin that applies this to every object of another
//! origin builds on it.
//!
//! The format is a thin framing over audited primitives, ChaCha20-Poly1305 and HKDF-SHA256
//! (from `ring`), arranged the way the `age` format arranges its payload: a per-object key
//! derived from a random salt, then the STREAM construction over 64 KiB chunks. Unlike `age`,
//! the header has a fixed length, so an object's plaintext size follows from its ciphertext
//! size alone ([`plaintext_len`](crate::crypt::plaintext_len)): listing a directory needs no read of each object.
//!
//! # Format specification (v1)
//!
//! This section is normative: with it, an object can be decrypted without nimbus.
//!
//! **Master key.** 32 bytes. Its text form, as kept in a key file, is
//! `nimbus-crypt-key-1:` followed by the 32 bytes in unpadded URL-safe base64 (43 characters).
//!
//! **Object layout.** `header (48) ‖ data chunks ‖ footer chunk (24)`.
//!
//! | Offset | Len | Field | Value |
//! |--------|-----|-------|-------|
//! | 0      | 4   | magic | ASCII `NMBC` |
//! | 4      | 1   | version | `1` |
//! | 5      | 1   | flags | `0`. Bit 0 is reserved for a future id-bound mode; a v1 reader refuses any set bit |
//! | 6      | 2   | reserved | zero; a reader refuses anything else |
//! | 8      | 8   | key id | first 8 bytes of HKDF-SHA256(salt = empty, IKM = master key, info = `nimbus-crypt v1 key id`) |
//! | 16     | 32  | salt | fresh from the OS RNG for every encryption |
//!
//! **Payload key.** HKDF-SHA256(salt = the header's salt, IKM = master key,
//! info = `nimbus-crypt v1 payload` ‖ the whole 48-byte header), 32 bytes. Because the header is
//! part of `info`, changing any header byte derives a different key, so every chunk fails to
//! authenticate: the header is authenticated without a MAC of its own.
//!
//! **Chunks.** ChaCha20-Poly1305 (RFC 8439) under the payload key, empty associated data, each
//! chunk's ciphertext followed by its 16-byte tag. The 12-byte nonce of chunk *i* is
//! `0x00 × 7 ‖ i as u32 big-endian ‖ last`, where `last` is `0x01` for the footer chunk and
//! `0x00` for every data chunk (the STREAM construction, BE32 variant).
//!
//! - Data chunks carry the plaintext in order, 65536 bytes each; only the last one may be
//!   shorter (1 to 65536 bytes). An empty plaintext has no data chunks.
//! - The footer chunk always comes last and is the only chunk with `last = 0x01`. Its
//!   plaintext is the total plaintext length as a big-endian u64, so it is 24 bytes encrypted.
//!   A reader checks the length against what it decrypted.
//! - Chunk indices run 0, 1, … with the footer taking the index after the last data chunk.
//!   Indices are u32 and never wrap, so an object has at most 2^32 − 1 data chunks
//!   (just under 256 TiB of plaintext).
//!
//! **Sizes.** For a plaintext of P bytes the ciphertext is
//! `C = 48 + P + 16 · ceil(P / 65536) + 24` bytes. Conversely, with `body = C − 72` and
//! `n = ceil(body / 65552)`, `P = body − 16 · n`. A length is impossible, and the object
//! truncated or damaged, if `C < 72` or `body mod 65552` is between 1 and 16.
//!
//! **What a reader checks.** The magic (else not a nimbus-crypt object), the version, flags and
//! reserved bytes, every chunk's tag, that the last chunk and only the last carries the last
//! flag, and the footer's length. The key id only picks the error message: an authentication
//! failure under a different key id is reported as the wrong key, under the same one as damage.
//!
//! # What this does not hide or catch
//!
//! Object names, the directory tree, object counts, exact plaintext sizes and modification
//! times stay visible to whoever holds the storage. The key id is the same in every object
//! encrypted with one key, so it links objects (and vaults) that share a key; it reveals
//! nothing about the key itself. Names are not bound to content: someone who can write to the
//! storage can swap two objects' contents between names, or put back an older version of an
//! object (a rollback), and both still decrypt. Anything else — a flipped byte, a dropped,
//! duplicated or reordered chunk, a chunk from another object, a cut or an extension — fails.

mod error;
mod file;
mod format;
mod key;
mod stream;

pub use error::CryptError;
pub use file::{create_key_file, decrypt_file, write_key_file};
pub use format::{ciphertext_len, plaintext_len};
pub use key::CryptKey;
pub use stream::{decrypt, decrypt_bytes, encrypt, encrypt_bytes};

#[cfg(test)]
#[path = "tests/vectors.rs"]
mod vectors;
