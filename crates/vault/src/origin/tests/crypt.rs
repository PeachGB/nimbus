use super::*;
use crate::{origin::fs::OriginFileSystem, vault::Vault};
use std::{path::Path, sync::Arc};

/// A byte string no ciphertext should ever contain.
const PLAINTEXT_MARKER: &[u8] = b"NIMBUS-PLAINTEXT-MARKER";

fn key(byte: u8) -> CryptKey {
    CryptKey::from_bytes([byte; 32])
}

fn crypt_over(dir: &Path, key: CryptKey) -> OriginCrypt {
    OriginCrypt::new(Box::new(OriginFileSystem::new(dir.to_path_buf())), key)
}

fn leaf(name: &str) -> Object {
    Object::Leaf {
        name: name.to_string(),
        id: ObjectId::from(name),
        meta: Metadata::new(),
    }
}

fn bytes_stream(bytes: &[u8]) -> ByteStream {
    futures::stream::iter([Ok(Bytes::copy_from_slice(bytes))]).boxed()
}

/// Creates `name` at the root of `origin` with `contents`, returning the stored object.
async fn write(origin: &OriginCrypt, name: &str, contents: &[u8]) -> Object {
    let stored = origin
        .put(&mut leaf(name), &ObjectId::from(ROOT_ID))
        .await
        .unwrap();
    origin.send(&stored, bytes_stream(contents)).await.unwrap();
    stored
}

async fn read(origin: &dyn Origin, id: &ObjectId) -> VaultResult<Vec<u8>> {
    let parts: Vec<Bytes> = origin.fetch(id).await?.try_collect().await?;
    Ok(parts.concat())
}

/// Every regular file under `dir`, recursively.
fn files_under(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(files_under(&path));
        } else {
            out.push(path);
        }
    }
    out
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

// --- reading and writing ---

#[tokio::test]
async fn what_is_written_reads_back_and_is_ciphertext_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let origin = crypt_over(dir.path(), key(1));
    let mut contents = PLAINTEXT_MARKER.repeat(5000); // more than one chunk
    contents.extend_from_slice(b"tail");
    let stored = write(&origin, "notes.txt", &contents).await;

    assert_eq!(read(&origin, &stored.get_id()).await.unwrap(), contents);
    for file in files_under(dir.path()) {
        let bytes = std::fs::read(&file).unwrap();
        assert!(
            !contains(&bytes, PLAINTEXT_MARKER),
            "{} holds plaintext",
            file.display()
        );
        assert!(
            !contains(&bytes, b"tail"),
            "{} holds plaintext",
            file.display()
        );
    }
}

#[tokio::test]
async fn listings_and_get_report_plaintext_sizes() {
    let dir = tempfile::tempdir().unwrap();
    let origin = crypt_over(dir.path(), key(1));
    let stored = write(&origin, "a.bin", &[7; 70_000]).await;
    std::fs::create_dir(dir.path().join("sub")).unwrap();

    let on_disk = std::fs::metadata(dir.path().join("a.bin")).unwrap().len();
    assert_eq!(on_disk, crate::crypt::ciphertext_len(70_000).unwrap());
    let got = origin.get(&stored.get_id()).await.unwrap();
    assert_eq!(got.get_meta().unwrap().size, Some(70_000));
    let listed = origin.list(&ObjectId::from(ROOT_ID)).await.unwrap();
    let a = listed.iter().find(|o| o.get_name() == "a.bin").unwrap();
    assert_eq!(a.get_meta().unwrap().size, Some(70_000));
    // Directories pass through as they are.
    assert!(
        listed
            .iter()
            .any(|o| matches!(o, Object::Branch { name, .. } if name == "sub"))
    );
}

#[tokio::test]
async fn the_modified_time_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let origin = crypt_over(dir.path(), key(1));
    let stored = origin
        .put(&mut leaf("dated.txt"), &ObjectId::from(ROOT_ID))
        .await
        .unwrap();
    let when = chrono::DateTime::parse_from_rfc3339("2020-01-02T03:04:05Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let mut dated = stored.clone();
    let mut meta = Metadata::new();
    meta.set_modified(when);
    dated.set_meta(meta);
    origin
        .send(&dated, bytes_stream(b"old news"))
        .await
        .unwrap();

    let got = origin.get(&stored.get_id()).await.unwrap();
    assert_eq!(got.get_meta().unwrap().modified, Some(when));
}

#[tokio::test]
async fn put_stores_a_valid_empty_object_straight_away() {
    let dir = tempfile::tempdir().unwrap();
    let origin = crypt_over(dir.path(), key(1));
    let stored = origin
        .put(&mut leaf("new.txt"), &ObjectId::from(ROOT_ID))
        .await
        .unwrap();

    // Never a 0-byte object, even with no send after it.
    let on_disk = std::fs::read(dir.path().join("new.txt")).unwrap();
    assert_eq!(
        on_disk.len() as u64,
        crate::crypt::ciphertext_len(0).unwrap()
    );
    assert_eq!(stored.get_meta().unwrap().size, Some(0));
    assert_eq!(read(&origin, &stored.get_id()).await.unwrap(), b"");
}

#[tokio::test]
async fn every_send_uses_a_fresh_encryption() {
    let dir = tempfile::tempdir().unwrap();
    let origin = crypt_over(dir.path(), key(1));
    let stored = write(&origin, "same.txt", b"same").await;
    let first = std::fs::read(dir.path().join("same.txt")).unwrap();
    origin.send(&stored, bytes_stream(b"same")).await.unwrap();
    let second = std::fs::read(dir.path().join("same.txt")).unwrap();
    assert_ne!(first, second);
}

#[tokio::test]
async fn delete_and_rename_pass_through() {
    let dir = tempfile::tempdir().unwrap();
    let origin = crypt_over(dir.path(), key(1));
    let stored = write(&origin, "old.txt", b"moving").await;
    let moved = origin
        .rename(&stored.get_id(), &ObjectId::from(ROOT_ID), "new.txt")
        .await
        .unwrap();
    assert_eq!(moved.get_meta().unwrap().size, Some(6));
    assert_eq!(read(&origin, &moved.get_id()).await.unwrap(), b"moving");

    origin.delete(&moved.get_id()).await.unwrap();
    assert!(!dir.path().join("new.txt").exists());
}

// --- what isn't a valid object of this vault ---

#[tokio::test]
async fn a_plaintext_object_is_not_encrypted_never_passed_through() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("plain.txt"), b"hello").unwrap();
    let origin = crypt_over(dir.path(), key(1));
    let error = read(&origin, &ObjectId::from("plain.txt"))
        .await
        .unwrap_err();
    assert!(
        matches!(error, VaultError::Crypt(CryptError::NotEncrypted)),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_zero_byte_object_is_truncated_and_lists_with_an_unknown_size() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("empty.txt"), b"").unwrap();
    let origin = crypt_over(dir.path(), key(1));
    let id = ObjectId::from("empty.txt");

    let error = origin.get(&id).await.unwrap_err();
    assert!(
        matches!(error, VaultError::Crypt(CryptError::Truncated)),
        "{error:?}"
    );
    let error = read(&origin, &id).await.unwrap_err();
    assert!(
        matches!(error, VaultError::Crypt(CryptError::Truncated)),
        "{error:?}"
    );
    // One damaged object doesn't break the listing; its size is just unknown.
    let listed = origin.list(&ObjectId::from(ROOT_ID)).await.unwrap();
    let empty = listed.iter().find(|o| o.get_name() == "empty.txt").unwrap();
    assert_eq!(empty.get_meta().unwrap().size, None);
}

// --- the marker ---

#[tokio::test]
async fn the_first_write_leaves_a_marker_that_listings_hide() {
    let dir = tempfile::tempdir().unwrap();
    let origin = crypt_over(dir.path(), key(1));
    write(&origin, "a.txt", b"a").await;

    assert!(dir.path().join(MARKER_NAME).is_file());
    let names: Vec<String> = origin
        .list(&ObjectId::from(ROOT_ID))
        .await
        .unwrap()
        .iter()
        .map(Object::get_name)
        .collect();
    assert_eq!(names, vec!["a.txt".to_string()]);
    // A fresh origin (another process) hides it too, also under the "" spelling of the root.
    let again = crypt_over(dir.path(), key(1));
    let names: Vec<String> = again
        .list(&ObjectId::from(""))
        .await
        .unwrap()
        .iter()
        .map(Object::get_name)
        .collect();
    assert_eq!(names, vec!["a.txt".to_string()]);
}

#[tokio::test]
async fn reading_alone_writes_no_marker() {
    let dir = tempfile::tempdir().unwrap();
    let origin = crypt_over(dir.path(), key(1));
    origin.list(&ObjectId::from(ROOT_ID)).await.unwrap();
    assert!(!dir.path().join(MARKER_NAME).exists());
}

#[tokio::test]
async fn another_key_is_refused_before_anything_is_written_or_read() {
    let dir = tempfile::tempdir().unwrap();
    let first = crypt_over(dir.path(), key(1));
    let stored = write(&first, "a.txt", b"a").await;
    let before = std::fs::read(dir.path().join("a.txt")).unwrap();

    let second = crypt_over(dir.path(), key(2));
    let wrong_key = |error: VaultError| matches!(error, VaultError::Crypt(CryptError::WrongKey));
    let mut new = leaf("b.txt");
    assert!(wrong_key(
        second
            .put(&mut new, &ObjectId::from(ROOT_ID))
            .await
            .unwrap_err()
    ));
    assert!(wrong_key(
        second.send(&stored, bytes_stream(b"x")).await.unwrap_err()
    ));
    assert!(wrong_key(
        second.delete(&stored.get_id()).await.unwrap_err()
    ));
    assert!(wrong_key(
        second.list(&ObjectId::from(ROOT_ID)).await.unwrap_err()
    ));
    assert!(wrong_key(second.get(&stored.get_id()).await.unwrap_err()));
    assert!(wrong_key(
        read(&second, &stored.get_id()).await.unwrap_err()
    ));

    assert_eq!(std::fs::read(dir.path().join("a.txt")).unwrap(), before);
    assert!(!dir.path().join("b.txt").exists());
}

// --- sync through a real vault ---

fn fs_vault(name: &str, root: &Path) -> Vault {
    Vault::from_parts(
        name.to_string(),
        Arc::new(OriginFileSystem::new(root.to_path_buf())),
        ObjectId::from(""),
    )
    .unwrap()
}

#[tokio::test]
async fn pushing_into_a_crypt_vault_copies_once() {
    let tmp = tempfile::tempdir().unwrap();
    let (local, remote) = (tmp.path().join("local"), tmp.path().join("remote"));
    std::fs::create_dir_all(local.join("docs")).unwrap();
    std::fs::create_dir_all(&remote).unwrap();
    std::fs::write(local.join("a.txt"), b"alpha").unwrap();
    std::fs::write(local.join("docs/b.txt"), PLAINTEXT_MARKER).unwrap();
    let plain = fs_vault("local", &local);
    let crypt = crypt_over(&remote, key(1));

    let first = plain.push(&ObjectId::from(""), &crypt).await.unwrap();
    assert!(first.copied.len() >= 2, "{first:?}");
    assert!(first.conflicts.is_empty(), "{first:?}");
    let second = plain.push(&ObjectId::from(""), &crypt).await.unwrap();
    assert!(
        second.copied.is_empty(),
        "the second push copied {:?}",
        second.copied
    );

    for file in files_under(&remote) {
        assert!(!contains(&std::fs::read(&file).unwrap(), PLAINTEXT_MARKER));
    }
    assert_eq!(
        read(&crypt, &ObjectId::from("docs/b.txt")).await.unwrap(),
        PLAINTEXT_MARKER
    );
}

/// A local vault with `a.txt` (old contents, old time) and an encrypted remote holding a newer
/// `a.txt` of more than one chunk, so a pull would replace the local one.
async fn local_and_newer_remote(tmp: &Path) -> (Vault, std::path::PathBuf, OriginCrypt, Vec<u8>) {
    let (local, remote) = (tmp.join("local"), tmp.join("remote"));
    std::fs::create_dir_all(&local).unwrap();
    std::fs::create_dir_all(&remote).unwrap();
    std::fs::write(local.join("a.txt"), b"old local contents").unwrap();
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
    std::fs::File::options()
        .write(true)
        .open(local.join("a.txt"))
        .unwrap()
        .set_modified(old)
        .unwrap();
    let crypt = crypt_over(&remote, key(1));
    write(&crypt, "a.txt", &[b'n'; 150_000]).await;
    let before = std::fs::read(local.join("a.txt")).unwrap();
    (fs_vault("local", &local), local, crypt, before)
}

/// The local directory's file names, temp files included.
fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn a_pull_from_a_tampered_or_truncated_remote_leaves_the_local_copy_untouched() {
    for damage in [
        "flip a byte in the last chunk",
        "cut off the footer",
        "append bytes",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let (vault, local, crypt, before) = local_and_newer_remote(tmp.path()).await;
        let stored = tmp.path().join("remote/a.txt");
        let mut bytes = std::fs::read(&stored).unwrap();
        match damage {
            "flip a byte in the last chunk" => {
                let at = bytes.len() - 40;
                bytes[at] ^= 1;
            }
            "cut off the footer" => bytes.truncate(bytes.len() - 24),
            _ => bytes.extend_from_slice(&[0; 100]),
        }
        std::fs::write(&stored, &bytes).unwrap();

        // Data chunks are released before the end is verified: the destination must discard
        // them when the stream ends in an error.
        let outcome = vault.pull(&ObjectId::from(""), &crypt).await;
        assert!(
            outcome.is_err() || !outcome.as_ref().unwrap().conflicts.is_empty(),
            "{damage}: {outcome:?}"
        );
        assert_eq!(
            std::fs::read(local.join("a.txt")).unwrap(),
            before,
            "{damage}"
        );
        assert_eq!(
            names_in(&local),
            vec!["a.txt".to_string()],
            "{damage}: leftovers"
        );
    }
}

#[tokio::test]
async fn a_pull_with_the_wrong_key_is_an_error_never_a_copy() {
    let tmp = tempfile::tempdir().unwrap();
    let (vault, local, _, before) = local_and_newer_remote(tmp.path()).await;
    let wrong = crypt_over(&tmp.path().join("remote"), key(2));

    let error = vault.pull(&ObjectId::from(""), &wrong).await.unwrap_err();
    assert!(
        matches!(error, VaultError::Crypt(CryptError::WrongKey)),
        "{error:?}"
    );
    assert_eq!(std::fs::read(local.join("a.txt")).unwrap(), before);
}

#[tokio::test]
async fn a_pull_from_an_intact_remote_replaces_the_local_copy() {
    // The control for the damage test above: undamaged, the same pull does copy.
    let tmp = tempfile::tempdir().unwrap();
    let (vault, local, crypt, _) = local_and_newer_remote(tmp.path()).await;
    let report = vault.pull(&ObjectId::from(""), &crypt).await.unwrap();
    assert_eq!(report.copied.len(), 1, "{report:?}");
    assert_eq!(
        std::fs::read(local.join("a.txt")).unwrap(),
        vec![b'n'; 150_000]
    );
}

/// An fs origin that records the size of every object it is asked to `send`, the way a command
/// origin's `{size}` would see it.
struct SizeSpy {
    inner: OriginFileSystem,
    sent: Arc<std::sync::Mutex<Vec<Option<u64>>>>,
}

#[async_trait::async_trait]
impl Origin for SizeSpy {
    async fn fetch(&self, id: &ObjectId) -> VaultResult<ByteStream> {
        self.inner.fetch(id).await
    }
    async fn list(&self, id: &ObjectId) -> VaultResult<Vec<Object>> {
        self.inner.list(id).await
    }
    async fn get(&self, id: &ObjectId) -> VaultResult<Object> {
        self.inner.get(id).await
    }
    async fn put(&self, object: &mut Object, destination: &ObjectId) -> VaultResult<Object> {
        self.inner.put(object, destination).await
    }
    async fn send(&self, object: &Object, payload: ByteStream) -> VaultResult<()> {
        let size = object.get_meta().and_then(|meta| meta.size);
        self.sent.lock().unwrap().push(size);
        self.inner.send(object, payload).await
    }
    async fn delete(&self, id: &ObjectId) -> VaultResult<()> {
        self.inner.delete(id).await
    }
}

#[tokio::test]
async fn the_inner_origin_is_told_the_ciphertext_size() {
    let dir = tempfile::tempdir().unwrap();
    let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
    let spy = SizeSpy {
        inner: OriginFileSystem::new(dir.path().to_path_buf()),
        sent: sent.clone(),
    };
    let origin = OriginCrypt::new(Box::new(spy), key(1));
    let stored = origin
        .put(&mut leaf("sized.txt"), &ObjectId::from(ROOT_ID))
        .await
        .unwrap();
    let mut sized = stored.clone();
    let mut meta = Metadata::new();
    meta.set_size(1000);
    sized.set_meta(meta);
    origin.send(&sized, bytes_stream(&[0; 1000])).await.unwrap();

    let last = *sent.lock().unwrap().last().unwrap();
    assert_eq!(last, crate::crypt::ciphertext_len(1000));
}

// --- overwrites, the reserved marker, racing first writers ---

#[tokio::test]
async fn an_overwrite_whose_send_fails_keeps_the_old_contents() {
    let dir = tempfile::tempdir().unwrap();
    let origin = crypt_over(dir.path(), key(1));
    write(&origin, "a.txt", b"hello original").await;

    // An overwrite is a put then a send; here the send's payload fails halfway.
    let stored = origin
        .put(&mut leaf("a.txt"), &ObjectId::from(ROOT_ID))
        .await
        .unwrap();
    let failing = futures::stream::iter([
        Ok(Bytes::from_static(b"new partial")),
        Err(VaultError::Generic("connection lost".into())),
    ])
    .boxed();
    assert!(origin.send(&stored, failing).await.is_err());

    assert_eq!(
        read(&origin, &stored.get_id()).await.unwrap(),
        b"hello original"
    );
    // And put reported the existing object as it is.
    assert_eq!(stored.get_meta().unwrap().size, Some(14));
}

fn is_reserved(error: VaultError) -> bool {
    error.to_string().contains("reserved by nimbus-crypt")
}

#[tokio::test]
async fn the_marker_cannot_be_replaced_moved_or_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let origin = crypt_over(dir.path(), key(1));
    let a = write(&origin, "a.txt", b"a").await;
    let marker_before = std::fs::read(dir.path().join(MARKER_NAME)).unwrap();
    let marker_id = ObjectId::from(MARKER_NAME);

    let mut fake = leaf(MARKER_NAME);
    for root in ["/", ""] {
        let error = origin
            .put(&mut fake, &ObjectId::from(root))
            .await
            .unwrap_err();
        assert!(is_reserved(error), "put under {root:?}");
    }
    let marker_object = Object::Leaf {
        name: MARKER_NAME.into(),
        id: marker_id.clone(),
        meta: Metadata::new(),
    };
    assert!(is_reserved(
        origin
            .send(&marker_object, bytes_stream(b"x"))
            .await
            .unwrap_err()
    ));
    assert!(is_reserved(origin.delete(&marker_id).await.unwrap_err()));
    assert!(is_reserved(
        origin
            .delete(&ObjectId::from("/.nimbus-crypt"))
            .await
            .unwrap_err()
    ));
    assert!(is_reserved(
        origin
            .rename(&a.get_id(), &ObjectId::from(ROOT_ID), MARKER_NAME)
            .await
            .unwrap_err()
    ));
    assert!(is_reserved(
        origin
            .rename(&marker_id, &ObjectId::from(ROOT_ID), "elsewhere")
            .await
            .unwrap_err()
    ));

    assert_eq!(
        std::fs::read(dir.path().join(MARKER_NAME)).unwrap(),
        marker_before
    );
    // A fresh session with the same key still opens the vault.
    let again = crypt_over(dir.path(), key(1));
    assert_eq!(again.list(&ObjectId::from(ROOT_ID)).await.unwrap().len(), 1);
}

/// An fs origin where another writer, with another key, puts its marker in place right after
/// ours is written: the race between two first writers, made deterministic.
struct LosesTheRace {
    inner: OriginFileSystem,
    root: std::path::PathBuf,
}

#[async_trait::async_trait]
impl Origin for LosesTheRace {
    async fn fetch(&self, id: &ObjectId) -> VaultResult<ByteStream> {
        self.inner.fetch(id).await
    }
    async fn list(&self, id: &ObjectId) -> VaultResult<Vec<Object>> {
        self.inner.list(id).await
    }
    async fn get(&self, id: &ObjectId) -> VaultResult<Object> {
        self.inner.get(id).await
    }
    async fn put(&self, object: &mut Object, destination: &ObjectId) -> VaultResult<Object> {
        self.inner.put(object, destination).await
    }
    async fn send(&self, object: &Object, payload: ByteStream) -> VaultResult<()> {
        self.inner.send(object, payload).await?;
        if object.get_name() == MARKER_NAME {
            let theirs = crate::crypt::encrypt_bytes(&key(2), MARKER_PLAINTEXT).unwrap();
            std::fs::write(self.root.join(MARKER_NAME), theirs).unwrap();
        }
        Ok(())
    }
    async fn delete(&self, id: &ObjectId) -> VaultResult<()> {
        self.inner.delete(id).await
    }
}

#[tokio::test]
async fn a_first_writer_that_loses_the_marker_race_refuses_to_write() {
    let dir = tempfile::tempdir().unwrap();
    let racing = LosesTheRace {
        inner: OriginFileSystem::new(dir.path().to_path_buf()),
        root: dir.path().to_path_buf(),
    };
    let origin = OriginCrypt::new(Box::new(racing), key(1));

    let error = origin
        .put(&mut leaf("mine.txt"), &ObjectId::from(ROOT_ID))
        .await
        .unwrap_err();
    assert!(
        matches!(error, VaultError::Crypt(CryptError::WrongKey)),
        "{error:?}"
    );
    assert!(!dir.path().join("mine.txt").exists());
}

#[tokio::test]
async fn dot_spellings_of_the_marker_are_refused_too() {
    let dir = tempfile::tempdir().unwrap();
    let origin = crypt_over(dir.path(), key(1));
    let a = write(&origin, "a.txt", b"a").await;
    let marker_before = std::fs::read(dir.path().join(MARKER_NAME)).unwrap();

    for spelling in [
        "./.nimbus-crypt",
        ".//.nimbus-crypt",
        "/./.nimbus-crypt",
        "./././.nimbus-crypt",
    ] {
        let id = ObjectId::from(spelling);
        let object = Object::Leaf {
            name: MARKER_NAME.into(),
            id: id.clone(),
            meta: Metadata::new(),
        };
        assert!(
            is_reserved(origin.send(&object, bytes_stream(b"x")).await.unwrap_err()),
            "send {spelling}"
        );
        assert!(
            is_reserved(origin.delete(&id).await.unwrap_err()),
            "delete {spelling}"
        );
        assert!(
            is_reserved(
                origin
                    .rename(&id, &ObjectId::from(ROOT_ID), "moved")
                    .await
                    .unwrap_err()
            ),
            "rename from {spelling}"
        );
    }
    for parent in [".", "./", "/.", ".//"] {
        let error = origin
            .rename(&a.get_id(), &ObjectId::from(parent), MARKER_NAME)
            .await
            .unwrap_err();
        assert!(is_reserved(error), "rename into {parent:?}");
        let error = origin
            .put(&mut leaf(MARKER_NAME), &ObjectId::from(parent))
            .await
            .unwrap_err();
        assert!(is_reserved(error), "put into {parent:?}");
    }

    assert_eq!(
        std::fs::read(dir.path().join(MARKER_NAME)).unwrap(),
        marker_before
    );
    let again = crypt_over(dir.path(), key(1));
    assert_eq!(again.list(&ObjectId::from(ROOT_ID)).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_parent_segment_is_refused_in_any_write_whatever_the_inner_origin_does() {
    let dir = tempfile::tempdir().unwrap();
    let origin = crypt_over(dir.path(), key(1));
    let a = write(&origin, "a.txt", b"a").await;
    std::fs::create_dir(dir.path().join("sub")).unwrap();

    let sneaky = ObjectId::from("sub/../.nimbus-crypt");
    let invalid = |error: VaultError| matches!(error, VaultError::InvalidId(_));
    let object = Object::Leaf {
        name: MARKER_NAME.into(),
        id: sneaky.clone(),
        meta: Metadata::new(),
    };
    assert!(invalid(
        origin.send(&object, bytes_stream(b"x")).await.unwrap_err()
    ));
    assert!(invalid(origin.delete(&sneaky).await.unwrap_err()));
    assert!(invalid(
        origin
            .rename(&a.get_id(), &ObjectId::from("sub/.."), MARKER_NAME)
            .await
            .unwrap_err()
    ));
    let again = crypt_over(dir.path(), key(1));
    assert_eq!(again.list(&ObjectId::from(ROOT_ID)).await.unwrap().len(), 2);
}

#[tokio::test]
async fn a_parent_segment_never_reaches_the_inner_origin() {
    // Not fs's own refusal: crypt refuses `..` before the inner origin is even called, so the
    // marker is safe over an origin that would resolve it.
    let dir = tempfile::tempdir().unwrap();
    let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
    let spy = SizeSpy {
        inner: OriginFileSystem::new(dir.path().to_path_buf()),
        sent: sent.clone(),
    };
    let origin = OriginCrypt::new(Box::new(spy), key(1));
    write(&origin, "a.txt", b"a").await;
    let sends_before = sent.lock().unwrap().len();

    let sneaky = Object::Leaf {
        name: MARKER_NAME.into(),
        id: ObjectId::from("sub/../.nimbus-crypt"),
        meta: Metadata::new(),
    };
    let error = origin.send(&sneaky, bytes_stream(b"x")).await.unwrap_err();
    assert!(matches!(error, VaultError::InvalidId(_)), "{error:?}");
    assert_eq!(sent.lock().unwrap().len(), sends_before);
}

#[tokio::test]
async fn the_marker_is_refused_however_a_filesystem_might_spell_it() {
    // Identical on every platform: case-insensitive filesystems (macOS, Windows), `\` as a
    // separator and trailing dots and spaces stripped (Windows) all name the marker.
    let dir = tempfile::tempdir().unwrap();
    let origin = crypt_over(dir.path(), key(1));
    let a = write(&origin, "a.txt", b"a").await;
    let marker_before = std::fs::read(dir.path().join(MARKER_NAME)).unwrap();
    let reserved = |error: VaultError| matches!(error, VaultError::Reserved(_));

    for spelling in [
        ".NIMBUS-CRYPT",
        ".Nimbus-Crypt",
        ".\\.nimbus-crypt",
        ".nimbus-crypt.",
        ".nimbus-crypt ",
        ".\\.NIMBUS-crypt. .",
    ] {
        let id = ObjectId::from(spelling);
        let object = Object::Leaf {
            name: spelling.into(),
            id: id.clone(),
            meta: Metadata::new(),
        };
        assert!(
            reserved(origin.send(&object, bytes_stream(b"x")).await.unwrap_err()),
            "send {spelling:?}"
        );
        assert!(
            reserved(origin.delete(&id).await.unwrap_err()),
            "delete {spelling:?}"
        );
        assert!(
            reserved(
                origin
                    .rename(&id, &ObjectId::from(ROOT_ID), "moved")
                    .await
                    .unwrap_err()
            ),
            "rename from {spelling:?}"
        );
        assert!(
            reserved(
                origin
                    .rename(&a.get_id(), &ObjectId::from(ROOT_ID), spelling)
                    .await
                    .unwrap_err()
            ),
            "rename onto {spelling:?}"
        );
        assert!(
            reserved(
                origin
                    .put(&mut leaf(spelling), &ObjectId::from(ROOT_ID))
                    .await
                    .unwrap_err()
            ),
            "put {spelling:?}"
        );
    }
    // A Windows-style parent spelling of the root, too.
    assert!(reserved(
        origin
            .put(&mut leaf(MARKER_NAME), &ObjectId::from(".\\"))
            .await
            .unwrap_err()
    ));
    // Dot-and-space components that could mean `..` are refused outright.
    for odd in ["...", ". .", ".. ", "a\\..\\.nimbus-crypt"] {
        let error = origin.delete(&ObjectId::from(odd)).await.unwrap_err();
        assert!(
            matches!(error, VaultError::InvalidId(_)),
            "{odd:?}: {error:?}"
        );
    }

    assert_eq!(
        std::fs::read(dir.path().join(MARKER_NAME)).unwrap(),
        marker_before
    );
    let again = crypt_over(dir.path(), key(1));
    assert_eq!(again.list(&ObjectId::from(ROOT_ID)).await.unwrap().len(), 1);
}
