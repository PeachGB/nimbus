//! Streaming encryption and decryption: [`encrypt`] and [`decrypt`] turn a byte stream into
//! the other, holding at most one chunk (64 KiB plus a little) in memory at a time.

use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use ring::aead::{self, Aad, Nonce};

use super::{
    CryptError, CryptKey,
    format::{
        CHUNK_LEN, FOOTER_LEN, HEADER_LEN, Header, SALT_LEN, SEALED_CHUNK_LEN, SEALED_FOOTER_LEN,
        TAG_LEN, could_be_magic, nonce,
    },
    key::fill_random,
};
use crate::{error::VaultError, origin::ByteStream};

/// Encrypts `plaintext` into a nimbus-crypt object, as a stream. The salt is drawn from the OS
/// random number generator here, once per call, so every encryption (every send, every
/// overwrite) gets a fresh payload key; that is the only failure this call itself can report.
/// Errors from `plaintext` pass through, and end the stream.
///
/// # Examples
///
/// ```
/// use futures::{StreamExt, TryStreamExt, stream};
/// use nimbus_vault::crypt::{CryptKey, decrypt, encrypt};
///
/// # futures::executor::block_on(async {
/// let key = CryptKey::generate()?;
/// let plain = stream::iter([Ok(bytes::Bytes::from_static(b"hello"))]).boxed();
/// let sealed = encrypt(&key, plain)?;
/// let opened: Vec<_> = decrypt(&key, sealed).try_collect().await?;
/// assert_eq!(opened.concat(), b"hello");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn encrypt(key: &CryptKey, plaintext: ByteStream) -> Result<ByteStream, CryptError> {
    let mut salt = [0u8; SALT_LEN];
    fill_random(&mut salt)?;
    Ok(encrypt_with_salt(key, salt, plaintext))
}

/// [`encrypt`] with a given salt. Only for the frozen test vectors: a real encryption must
/// never choose its salt.
pub(crate) fn encrypt_with_salt(
    key: &CryptKey,
    salt: [u8; SALT_LEN],
    plaintext: ByteStream,
) -> ByteStream {
    let header = Header {
        key_id: key.key_id(),
        salt,
    }
    .encode();
    let state = Encrypting {
        sealer: Sealer::new(key.payload_key(&header), 0),
        header: Some(header),
        input: Some(plaintext),
        pending: BytesMut::new(),
        total: 0,
        done: false,
    };
    futures::stream::unfold(state, |mut state| async move {
        state.next().await.map(|item| (item, state))
    })
    .boxed()
}

/// Decrypts a nimbus-crypt object, as a stream. Every chunk is authenticated before any of its
/// plaintext is yielded; an error (wrong key, damage, truncation) ends the stream with
/// [`VaultError::Crypt`], possibly after earlier chunks were yielded, so a consumer must treat
/// the whole result as void on error, as an atomic write does.
pub fn decrypt(key: &CryptKey, ciphertext: ByteStream) -> ByteStream {
    let state = Decrypting {
        key: key.clone(),
        opener: None,
        input: Some(ciphertext),
        buf: BytesMut::new(),
        done: false,
    };
    futures::stream::unfold(state, |mut state| async move {
        state.next().await.map(|item| (item, state))
    })
    .boxed()
}

/// [`encrypt`] for a small value held in memory. It blocks the calling thread until done, so
/// it is for small in-memory values outside async code; in async code use [`encrypt`].
pub fn encrypt_bytes(key: &CryptKey, plaintext: &[u8]) -> Result<Vec<u8>, CryptError> {
    let input = futures::stream::iter([Ok(Bytes::copy_from_slice(plaintext))]).boxed();
    collect(encrypt(key, input)?)
}

/// [`decrypt`] for a small object held in memory. It blocks the calling thread until done, so
/// it is for small in-memory values outside async code; in async code use [`decrypt`].
pub fn decrypt_bytes(key: &CryptKey, ciphertext: &[u8]) -> Result<Vec<u8>, CryptError> {
    let input = futures::stream::iter([Ok(Bytes::copy_from_slice(ciphertext))]).boxed();
    collect(decrypt(key, input))
}

/// Drains a stream that only ever yields crypt errors (its input can't fail).
fn collect(stream: ByteStream) -> Result<Vec<u8>, CryptError> {
    futures::executor::block_on(async {
        let mut out = Vec::new();
        let mut stream = stream;
        while let Some(item) = stream.next().await {
            match item {
                Ok(bytes) => out.extend_from_slice(&bytes),
                Err(VaultError::Crypt(error)) => return Err(error),
                Err(_) => unreachable!("an in-memory input can't fail"),
            }
        }
        Ok(out)
    })
}

/// Seals chunks in order under one payload key, numbering them and never letting the 32-bit
/// counter wrap.
struct Sealer {
    key: aead::LessSafeKey,
    /// Index of the next chunk; `None` once the footer (the last index) was sealed.
    next: Option<u32>,
}

impl Sealer {
    fn new(key: aead::LessSafeKey, first: u32) -> Self {
        Sealer {
            key,
            next: Some(first),
        }
    }

    /// Seals `chunk` in place (appending its tag). A data chunk needs an index after it for the
    /// footer, so it fails with [`CryptError::TooLarge`] at the counter's limit.
    fn seal(&mut self, chunk: &mut Vec<u8>, last: bool) -> Result<(), CryptError> {
        let index = self.next.ok_or(CryptError::TooLarge)?;
        self.next = if last {
            None
        } else {
            Some(index.checked_add(1).ok_or(CryptError::TooLarge)?)
        };
        self.key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce(index, last)),
                Aad::empty(),
                chunk,
            )
            .map_err(|_| CryptError::TooLarge)
    }
}

/// Opens chunks in order under one payload key. Knows whether the header's key id was this
/// key's, which is what tells a wrong key from damage when a chunk fails.
struct Opener {
    key: aead::LessSafeKey,
    next: u32,
    key_id_matches: bool,
    plaintext_len: u64,
}

impl Opener {
    /// Opens one sealed chunk, returning its plaintext.
    fn open(&mut self, sealed: &mut [u8], last: bool) -> Result<usize, CryptError> {
        let index = self.next;
        let failed = || {
            if self.key_id_matches {
                CryptError::Corrupt {
                    chunk: u64::from(index),
                }
            } else {
                CryptError::WrongKey
            }
        };
        let len = self
            .key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce(index, last)),
                Aad::empty(),
                sealed,
            )
            .map_err(|_| failed())?
            .len();
        // Only a data chunk moves the counter on, as in `Sealer::seal`: the footer may sit at
        // u32::MAX, and nothing comes after it.
        if !last {
            self.next = index.checked_add(1).ok_or_else(failed)?;
        }
        Ok(len)
    }
}

/// The state of an [`encrypt`] stream.
struct Encrypting {
    sealer: Sealer,
    /// The header, until it has been yielded.
    header: Option<[u8; HEADER_LEN]>,
    /// The plaintext, until it ends.
    input: Option<ByteStream>,
    /// Plaintext not yet sealed: less than a full chunk, except just before sealing.
    pending: BytesMut,
    total: u64,
    done: bool,
}

impl Encrypting {
    /// The next piece of ciphertext: the header, a data chunk, or finally the footer.
    async fn next(&mut self) -> Option<Result<Bytes, VaultError>> {
        if let Some(header) = self.header.take() {
            return Some(Ok(Bytes::copy_from_slice(&header)));
        }
        if self.done {
            return None;
        }
        loop {
            if self.pending.len() >= CHUNK_LEN {
                let chunk = self.pending.split_to(CHUNK_LEN);
                return Some(self.seal_data(&chunk));
            }
            let Some(input) = self.input.as_mut() else {
                return Some(self.finish());
            };
            match input.next().await {
                Some(Ok(bytes)) => self.pending.extend_from_slice(&bytes),
                Some(Err(error)) => {
                    self.done = true;
                    return Some(Err(error));
                }
                None => {
                    self.input = None;
                    if !self.pending.is_empty() {
                        let chunk = self.pending.split();
                        return Some(self.seal_data(&chunk));
                    }
                }
            }
        }
    }

    fn seal_data(&mut self, plaintext: &[u8]) -> Result<Bytes, VaultError> {
        self.total += plaintext.len() as u64;
        self.seal(plaintext, false)
    }

    /// The footer: the total length, under the last flag.
    fn finish(&mut self) -> Result<Bytes, VaultError> {
        self.done = true;
        self.seal(&self.total.to_be_bytes(), true)
    }

    fn seal(&mut self, plaintext: &[u8], last: bool) -> Result<Bytes, VaultError> {
        let mut chunk = Vec::with_capacity(plaintext.len() + TAG_LEN);
        chunk.extend_from_slice(plaintext);
        match self.sealer.seal(&mut chunk, last) {
            Ok(()) => Ok(Bytes::from(chunk)),
            Err(error) => {
                self.done = true;
                Err(error.into())
            }
        }
    }
}

/// The state of a [`decrypt`] stream.
struct Decrypting {
    key: CryptKey,
    /// Set once the header has been read.
    opener: Option<Opener>,
    /// The ciphertext, until it ends.
    input: Option<ByteStream>,
    /// Ciphertext read but not yet opened.
    buf: BytesMut,
    done: bool,
}

impl Decrypting {
    /// The next piece of plaintext, an error, or `None` once the footer checked out.
    async fn next(&mut self) -> Option<Result<Bytes, VaultError>> {
        if self.done {
            return None;
        }
        match self.step().await {
            Ok(Some(bytes)) => Some(Ok(bytes)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(error) => {
                self.done = true;
                Some(Err(error))
            }
        }
    }

    /// Reads until a data chunk can be opened (it is followed by at least a footer's worth of
    /// bytes, so it can't be the footer) or the input ends.
    async fn step(&mut self) -> Result<Option<Bytes>, VaultError> {
        loop {
            if self.opener.is_none() {
                if !could_be_magic(&self.buf) {
                    return Err(CryptError::NotEncrypted.into());
                }
                if self.buf.len() >= HEADER_LEN {
                    self.read_header()?;
                    continue;
                }
            } else if self.buf.len() >= SEALED_CHUNK_LEN + SEALED_FOOTER_LEN {
                let mut sealed = self.buf.split_to(SEALED_CHUNK_LEN);
                return self.open_data(&mut sealed).map(Some);
            }
            let Some(input) = self.input.as_mut() else {
                return self.finish();
            };
            match input.next().await {
                Some(Ok(bytes)) => self.buf.extend_from_slice(&bytes),
                Some(Err(error)) => return Err(error),
                None => self.input = None,
            }
        }
    }

    fn read_header(&mut self) -> Result<(), CryptError> {
        let bytes = self.buf.split_to(HEADER_LEN);
        let header: [u8; HEADER_LEN] = bytes[..].try_into().expect("split at HEADER_LEN");
        let parsed = Header::decode(&header)?;
        self.opener = Some(Opener {
            key: self.key.payload_key(&header),
            next: 0,
            key_id_matches: parsed.key_id == self.key.key_id(),
            plaintext_len: 0,
        });
        Ok(())
    }

    fn open_data(&mut self, sealed: &mut [u8]) -> Result<Bytes, VaultError> {
        let opener = self.opener.as_mut().expect("header read");
        let len = opener.open(sealed, false)?;
        opener.plaintext_len += len as u64;
        Ok(Bytes::copy_from_slice(&sealed[..len]))
    }

    /// At the end of the input: what's left is the last data chunk (if any) and the footer.
    /// Yields the last data chunk's plaintext, or `None` when there was none, after checking
    /// the footer either way. Runs once: the stream is done afterwards, whatever the outcome.
    fn finish(&mut self) -> Result<Option<Bytes>, VaultError> {
        self.done = true;
        if self.opener.is_none() {
            return Err(CryptError::Truncated.into());
        }
        let rest = self.buf.len();
        let data = rest
            .checked_sub(SEALED_FOOTER_LEN)
            .ok_or(CryptError::Truncated)?;
        if (1..=TAG_LEN).contains(&data) {
            return Err(CryptError::Truncated.into());
        }
        let mut last_data = self.buf.split_to(data);
        let plaintext = if data > 0 {
            Some(self.open_data(&mut last_data)?)
        } else {
            None
        };
        let mut footer = self.buf.split();
        let opener = self.opener.as_mut().expect("header read");
        let len = opener.open(&mut footer, true)?;
        let recorded: [u8; FOOTER_LEN] = footer[..len]
            .try_into()
            .map_err(|_| CryptError::LengthMismatch)?;
        if u64::from_be_bytes(recorded) != opener.plaintext_len {
            return Err(CryptError::LengthMismatch.into());
        }
        Ok(plaintext)
    }
}

#[cfg(test)]
#[path = "tests/stream.rs"]
mod tests;
