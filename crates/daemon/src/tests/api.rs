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

// --- regressions: hostile names, special characters in ids ---

#[tokio::test]
async fn put_with_a_name_that_escapes_the_vault_is_a_400_and_creates_nothing() {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("vault");
    std::fs::create_dir(&root).unwrap();
    let base = spawn(state_for(
        fs_vault("v", root.clone()),
        false,
        AuthConfig::None,
    ))
    .await;

    let client = reqwest::Client::new();
    for name in ["../escaped.txt", "a/b", ".."] {
        let response = client
            .put(format!("{base}{VAULT_PREFIX}/v/put/"))
            .json(&leaf(name))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            reqwest::StatusCode::BAD_REQUEST,
            "{name:?}"
        );
    }
    assert!(!outer.path().join("escaped.txt").exists());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
}

#[tokio::test]
async fn names_with_url_special_characters_round_trip_through_the_daemon() {
    // The scan's reproduction: sending `a#b` used to overwrite the unrelated object `a`.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a"), b"original").unwrap();
    let base = spawn(state_for(
        fs_vault("v", dir.path().to_path_buf()),
        false,
        AuthConfig::None,
    ))
    .await;
    let origin = origin_for(&base, "v");

    for name in ["a#b", "a?b", "50%", "sp ace", "ñandú"] {
        let stored = origin
            .put(&mut leaf(name), &ObjectId::from(""))
            .await
            .unwrap_or_else(|e| panic!("put {name:?}: {e}"));
        let body = format!("contents of {name}");
        let payload: nimbus_vault::origin::ByteStream = Box::pin(futures::stream::once({
            let body = body.clone();
            async move { Ok(bytes::Bytes::from(body)) }
        }));
        origin.send(&stored, payload).await.unwrap();

        assert_eq!(
            std::fs::read(dir.path().join(name)).unwrap(),
            body.as_bytes()
        );
        let fetched = collect(origin.fetch(&ObjectId::from(name)).await.unwrap()).await;
        assert_eq!(fetched, body.as_bytes(), "{name:?}");
    }
    assert_eq!(std::fs::read(dir.path().join("a")).unwrap(), b"original");
}

// --- regressions: modification times travel with the contents ---

#[tokio::test]
async fn send_applies_the_modified_header_and_pushing_twice_is_a_no_op() {
    let local_dir = tempfile::tempdir().unwrap();
    let served_dir = tempfile::tempdir().unwrap();
    std::fs::write(local_dir.path().join("notes.txt"), b"hello").unwrap();
    let base = spawn(state_for(
        fs_vault("v", served_dir.path().to_path_buf()),
        false,
        AuthConfig::None,
    ))
    .await;
    let remote = origin_for(&base, "v");
    let local = fs_vault("local", local_dir.path().to_path_buf());
    let root = ObjectId::from("");

    let first = local.push(&root, &remote).await.unwrap();
    assert_eq!(first.copied.len(), 1);
    let source_time = std::fs::metadata(local_dir.path().join("notes.txt"))
        .unwrap()
        .modified()
        .unwrap();
    let served_time = std::fs::metadata(served_dir.path().join("notes.txt"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(source_time, served_time);

    let second = local.push(&root, &remote).await.unwrap();
    assert!(second.copied.is_empty(), "{second:?}");
    assert_eq!(second.unchanged, 1);
}

#[tokio::test]
async fn send_with_a_malformed_modified_header_is_a_400() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"keep").unwrap();
    let base = spawn(state_for(
        fs_vault("v", dir.path().to_path_buf()),
        false,
        AuthConfig::None,
    ))
    .await;
    let response = reqwest::Client::new()
        .put(format!("{base}{VAULT_PREFIX}/v/send/a.txt"))
        .header(nimbus_vault::MODIFIED_HEADER, "yesterday")
        .body("new")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(std::fs::read(dir.path().join("a.txt")).unwrap(), b"keep");
}

#[cfg(unix)]
#[tokio::test]
async fn rename_route_moves_an_object_natively() {
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("docs/deep")).unwrap();
    std::fs::write(dir.path().join("docs/deep/a#1.txt"), b"x").unwrap();
    std::fs::create_dir(dir.path().join("archive")).unwrap();
    let inode = std::fs::metadata(dir.path().join("docs")).unwrap().ino();
    let base = spawn(state_for(
        fs_vault("v", dir.path().to_path_buf()),
        false,
        AuthConfig::None,
    ))
    .await;
    let origin = origin_for(&base, "v").with_rename_url(Some("/rename/{id}".to_string()));

    let moved = origin
        .rename(
            &ObjectId::from("docs"),
            &ObjectId::from("archive"),
            "old docs",
        )
        .await
        .unwrap();
    assert_eq!(moved.get_name(), "old docs");
    let target = dir.path().join("archive/old docs");
    assert_eq!(std::fs::metadata(&target).unwrap().ino(), inode);
    assert!(target.join("deep/a#1.txt").exists());
}

#[tokio::test]
async fn rename_route_rejects_hostile_names_and_destinations() {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("vault");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"x").unwrap();
    let base = spawn(state_for(
        fs_vault("v", root.clone()),
        false,
        AuthConfig::None,
    ))
    .await;
    let client = reqwest::Client::new();

    for body in [
        serde_json::json!({"destination": "", "name": "../out.txt"}),
        serde_json::json!({"destination": "../", "name": "out.txt"}),
    ] {
        let response = client
            .post(format!("{base}{VAULT_PREFIX}/v/rename/a.txt"))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            reqwest::StatusCode::BAD_REQUEST,
            "{body}"
        );
    }
    assert!(root.join("a.txt").exists());
    assert!(!outer.path().join("out.txt").exists());
}

#[tokio::test]
async fn rename_route_is_refused_when_read_only() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
    let base = spawn(state_for(
        fs_vault("v", dir.path().to_path_buf()),
        true,
        AuthConfig::None,
    ))
    .await;
    let response = reqwest::Client::new()
        .post(format!("{base}{VAULT_PREFIX}/v/rename/a.txt"))
        .json(&serde_json::json!({"destination": "", "name": "b.txt"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    assert!(dir.path().join("a.txt").exists());
}

// --- regression: malformed requests that aren't about ids are BadRequest, not InvalidId ---

#[test]
fn a_malformed_modified_header_is_a_bad_request() {
    let mut headers = HeaderMap::new();
    headers.insert(nimbus_vault::MODIFIED_HEADER, "yesterday".parse().unwrap());
    match modified_header(&headers) {
        Err(ApiError::BadRequest(reason)) => {
            assert!(reason.contains(nimbus_vault::MODIFIED_HEADER), "{reason}")
        }
        other => panic!("expected BadRequest, got {other:?}"),
    }
}

#[test]
fn a_valid_or_absent_modified_header_parses() {
    assert!(matches!(modified_header(&HeaderMap::new()), Ok(None)));
    let mut headers = HeaderMap::new();
    headers.insert(
        nimbus_vault::MODIFIED_HEADER,
        "2026-01-02T03:04:05.123456789Z".parse().unwrap(),
    );
    let parsed = modified_header(&headers).unwrap().unwrap();
    assert_eq!(parsed.to_rfc3339(), "2026-01-02T03:04:05.123456789+00:00");
}

#[tokio::test]
async fn bad_request_and_invalid_id_both_answer_400_with_their_reason() {
    use axum::response::IntoResponse;
    for error in [
        ApiError::BadRequest("bad header".to_string()),
        ApiError::InvalidId("bad id".to_string()),
    ] {
        assert_eq!(
            error.into_response().status(),
            axum::http::StatusCode::BAD_REQUEST
        );
    }
}
