use super::*;
use crate::auth::AuthConfig;
use nimbus_vault::{
    object::Metadata,
    origin::{
        Origin,
        fs::OriginFileSystem,
        http::{HttpAuth, OriginHTTP},
    },
};
use std::sync::Arc;

fn fs_vault(name: &str, root: std::path::PathBuf) -> Vault {
    Vault::from_parts(
        name.to_string(),
        Arc::new(OriginFileSystem::new(root)),
        ObjectId::from(""),
    )
    .unwrap()
}

fn state_for(vault: Vault, read_only: bool, auth: AuthConfig) -> AppState {
    let mut vaults = HashMap::new();
    vaults.insert(vault.get_name().clone(), vault);
    AppState::from_parts(vaults, read_only, auth)
}

async fn spawn(state: AppState) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = router(state);
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{address}")
}

fn origin_for(base: &str, vault: &str) -> OriginHTTP {
    OriginHTTP::new(
        format!("{base}{VAULT_PREFIX}/{vault}"),
        "/fetch/{id}".to_string(),
        "/list/{id}".to_string(),
        "/get/{id}".to_string(),
        "/put/{id}".to_string(),
        "/send/{id}".to_string(),
        "/delete/{id}".to_string(),
    )
}

fn leaf(name: &str) -> Object {
    Object::Leaf {
        name: name.to_string(),
        id: ObjectId::from(name),
        meta: Metadata::new(),
    }
}

async fn collect(stream: nimbus_vault::origin::ByteStream) -> Vec<u8> {
    stream
        .fold(Vec::new(), |mut acc, chunk| async move {
            acc.extend_from_slice(&chunk.unwrap());
            acc
        })
        .await
}

#[tokio::test]
async fn an_http_origin_can_read_a_served_vault() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    std::fs::write(dir.path().join("docs/notes.txt"), b"hello").unwrap();

    let base = spawn(state_for(
        fs_vault("photos", dir.path().to_path_buf()),
        false,
        AuthConfig::None,
    ))
    .await;
    let origin = origin_for(&base, "photos");

    let root = origin.list(&ObjectId::from("")).await.unwrap();
    assert_eq!(root.len(), 1);
    assert_eq!(root[0].get_name(), "docs");

    let object = origin.get(&ObjectId::from("docs/notes.txt")).await.unwrap();
    assert_eq!(object.get_name(), "notes.txt");
    assert!(matches!(object, Object::Leaf { .. }));

    let payload = collect(
        origin
            .fetch(&ObjectId::from("docs/notes.txt"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(payload, b"hello");
}

#[tokio::test]
async fn an_http_origin_can_write_to_a_served_vault() {
    let dir = tempfile::tempdir().unwrap();
    let base = spawn(state_for(
        fs_vault("photos", dir.path().to_path_buf()),
        false,
        AuthConfig::None,
    ))
    .await;
    let origin = origin_for(&base, "photos");

    let stored = origin
        .put(&mut leaf("notes.txt"), &ObjectId::from(""))
        .await
        .unwrap();
    assert_eq!(stored.get_name(), "notes.txt");
    assert!(dir.path().join("notes.txt").is_file());

    let payload = futures::stream::once(async { Ok(bytes_of("written by the daemon")) }).boxed();
    origin.send(&stored, payload).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("notes.txt")).unwrap(),
        "written by the daemon"
    );

    origin.delete(&ObjectId::from("notes.txt")).await.unwrap();
    assert!(!dir.path().join("notes.txt").exists());
}

fn bytes_of(text: &str) -> bytes::Bytes {
    bytes::Bytes::copy_from_slice(text.as_bytes())
}

#[tokio::test]
async fn a_client_rooted_at_the_default_slash_can_write_to_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let base = spawn(state_for(
        fs_vault("photos", dir.path().to_path_buf()),
        false,
        AuthConfig::None,
    ))
    .await;
    let origin = origin_for(&base, "photos");

    let stored = origin
        .put(&mut leaf("notes.txt"), &ObjectId::default())
        .await
        .unwrap();

    assert_eq!(stored.get_name(), "notes.txt");
    assert!(dir.path().join("notes.txt").is_file());
}

#[tokio::test]
async fn the_vault_root_is_reachable_however_the_client_spells_it() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"a").unwrap();
    let base = spawn(state_for(
        fs_vault("photos", dir.path().to_path_buf()),
        false,
        AuthConfig::None,
    ))
    .await;
    let client = reqwest::Client::new();

    for url in [
        format!("{base}{VAULT_PREFIX}/photos/list"),
        format!("{base}{VAULT_PREFIX}/photos/list/"),
        format!("{base}{VAULT_PREFIX}/photos/list//"),
    ] {
        let response = client.get(&url).send().await.unwrap();
        assert_eq!(response.status(), 200, "{url}");
        let objects: Vec<Object> = response.json().await.unwrap();
        assert_eq!(objects.len(), 1, "{url}");
    }
}

#[tokio::test]
async fn the_index_lists_the_served_vaults() {
    let dir = tempfile::tempdir().unwrap();
    let base = spawn(state_for(
        fs_vault("photos", dir.path().to_path_buf()),
        false,
        AuthConfig::None,
    ))
    .await;

    let names: Vec<String> = reqwest::get(format!("{base}{VAULT_PREFIX}"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(names, vec!["photos".to_string()]);
}

#[tokio::test]
async fn an_unknown_vault_is_a_404() {
    let dir = tempfile::tempdir().unwrap();
    let base = spawn(state_for(
        fs_vault("photos", dir.path().to_path_buf()),
        false,
        AuthConfig::None,
    ))
    .await;

    let response = reqwest::get(format!("{base}{VAULT_PREFIX}/nope/list"))
        .await
        .unwrap();

    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn a_missing_object_is_a_404_so_the_client_sees_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let base = spawn(state_for(
        fs_vault("photos", dir.path().to_path_buf()),
        false,
        AuthConfig::None,
    ))
    .await;
    let origin = origin_for(&base, "photos");

    let error = origin
        .get(&ObjectId::from("missing.txt"))
        .await
        .unwrap_err();

    assert!(
        matches!(error, VaultError::NotFound(_)),
        "expected NotFound, got {error:?}"
    );
}

#[tokio::test]
async fn a_read_only_daemon_still_serves_reads() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"a").unwrap();
    let base = spawn(state_for(
        fs_vault("photos", dir.path().to_path_buf()),
        true,
        AuthConfig::None,
    ))
    .await;
    let origin = origin_for(&base, "photos");

    assert_eq!(origin.list(&ObjectId::from("")).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_read_only_daemon_refuses_every_write() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"a").unwrap();
    let base = spawn(state_for(
        fs_vault("photos", dir.path().to_path_buf()),
        true,
        AuthConfig::None,
    ))
    .await;
    let client = reqwest::Client::new();

    for (method, url) in [
        (
            reqwest::Method::PUT,
            format!("{base}{VAULT_PREFIX}/photos/put/"),
        ),
        (
            reqwest::Method::PUT,
            format!("{base}{VAULT_PREFIX}/photos/send/a.txt"),
        ),
        (
            reqwest::Method::DELETE,
            format!("{base}{VAULT_PREFIX}/photos/delete/a.txt"),
        ),
    ] {
        let response = client
            .request(method, &url)
            .json(&leaf("b.txt"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403, "{url}");
    }
    assert!(dir.path().join("a.txt").exists());
}

#[tokio::test]
async fn a_protected_daemon_refuses_requests_without_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let base = spawn(state_for(
        fs_vault("photos", dir.path().to_path_buf()),
        false,
        AuthConfig::Bearer {
            token: "s3cr3t".to_string(),
        },
    ))
    .await;

    let response = reqwest::get(format!("{base}{VAULT_PREFIX}/photos/list"))
        .await
        .unwrap();

    assert_eq!(response.status(), 401);
    assert_eq!(response.headers()["www-authenticate"], "Bearer");
}

#[tokio::test]
async fn a_protected_daemon_serves_requests_carrying_the_token() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"a").unwrap();
    let base = spawn(state_for(
        fs_vault("photos", dir.path().to_path_buf()),
        false,
        AuthConfig::Bearer {
            token: "s3cr3t".to_string(),
        },
    ))
    .await;

    let response = reqwest::Client::new()
        .get(format!("{base}{VAULT_PREFIX}/photos/list"))
        .bearer_auth("s3cr3t")
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn a_vault_configured_with_the_token_can_use_a_protected_daemon() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"a").unwrap();
    let base = spawn(state_for(
        fs_vault("photos", dir.path().to_path_buf()),
        false,
        AuthConfig::Bearer {
            token: "s3cr3t".to_string(),
        },
    ))
    .await;

    let origin = origin_for(&base, "photos")
        .with_auth(&HttpAuth::Bearer {
            token: Some("s3cr3t".to_string()),
            token_env: None,
            token_file: None,
        })
        .unwrap();

    assert_eq!(origin.list(&ObjectId::from("")).await.unwrap().len(), 1);

    let error = origin_for(&base, "photos")
        .list(&ObjectId::from(""))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("401"), "{error}");
    assert!(error.contains("origin_config.auth"), "{error}");
}

#[tokio::test]
async fn health_answers_without_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let base = spawn(state_for(
        fs_vault("photos", dir.path().to_path_buf()),
        false,
        AuthConfig::Bearer {
            token: "s3cr3t".to_string(),
        },
    ))
    .await;

    let response = reqwest::get(format!("{base}/health")).await.unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), "ok");
}

#[test]
fn ordinary_ids_are_accepted_unchanged() {
    assert_eq!(
        object_id("docs/2024/notes.txt").unwrap(),
        ObjectId::from("docs/2024/notes.txt")
    );
    assert_eq!(object_id("notes.txt").unwrap(), ObjectId::from("notes.txt"));
    assert_eq!(object_id("/docs").unwrap(), ObjectId::from("/docs"));
}

#[test]
fn ids_that_climb_out_of_the_vault_are_refused() {
    for raw in [
        "../etc/passwd",
        "docs/../../etc/passwd",
        "..",
        "docs/..",
        "..\\windows",
    ] {
        assert!(
            matches!(object_id(raw), Err(ApiError::InvalidId(_))),
            "{raw} should have been refused"
        );
    }
}

#[test]
fn repeated_leading_slashes_are_collapsed_into_one() {
    assert_eq!(
        object_id("//notes.txt").unwrap(),
        ObjectId::from("/notes.txt")
    );
    assert_eq!(
        object_id("///etc/passwd").unwrap(),
        ObjectId::from("/etc/passwd")
    );
}

#[test]
fn ids_with_a_null_byte_are_refused() {
    assert!(matches!(
        object_id("notes\0.txt"),
        Err(ApiError::InvalidId(_))
    ));
}
