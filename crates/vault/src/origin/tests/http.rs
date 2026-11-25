use super::*;
use crate::object::Metadata;
use bytes::Bytes;
use futures::stream;
use httpmock::{Method, MockServer};

fn make_origin(server: &MockServer) -> OriginHTTP {
    OriginHTTP::new(
        server.base_url(),
        "/fetch/{id}".to_string(),
        "/list/{id}".to_string(),
        "/get/{id}".to_string(),
        "/put/{id}".to_string(),
        "/send/{id}".to_string(),
        "/delete/{id}".to_string(),
    )
}

#[test]
fn url_substitutes_id_into_template() {
    let origin = OriginHTTP::new(
        "http://x".to_string(),
        "/fetch/{id}".to_string(),
        "/list/{id}".to_string(),
        "/get/{id}".to_string(),
        "/put/{id}".to_string(),
        "/send/{id}".to_string(),
        "/delete/{id}".to_string(),
    );
    assert_eq!(
        origin.url(&origin.fetch_url, &ObjectId::from("obj1")),
        "http://x/fetch/obj1"
    );
}

#[test]
fn url_trims_trailing_slash_on_base() {
    let origin = OriginHTTP::new(
        "http://x/".to_string(),
        "/fetch/{id}".to_string(),
        "/list/{id}".to_string(),
        "/get/{id}".to_string(),
        "/put/{id}".to_string(),
        "/send/{id}".to_string(),
        "/delete/{id}".to_string(),
    );
    assert_eq!(
        origin.url(&origin.fetch_url, &ObjectId::from("obj1")),
        "http://x/fetch/obj1"
    );
}

#[tokio::test]
async fn fetch_streams_response_body() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(Method::GET).path("/fetch/f1");
        then.status(200).body("hello world");
    });
    let origin = make_origin(&server);

    let mut stream = origin.fetch(&ObjectId::from("f1")).await.unwrap();
    let mut collected = Vec::new();
    while let Some(chunk) = stream.next().await {
        collected.extend_from_slice(&chunk.unwrap());
    }
    assert_eq!(collected, b"hello world");
    mock.assert();
}

#[tokio::test]
async fn fetch_returns_not_found_on_404() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::GET).path("/fetch/missing");
        then.status(404);
    });
    let origin = make_origin(&server);

    let result = origin.fetch(&ObjectId::from("missing")).await;
    assert!(matches!(result, Err(VaultError::NotFound(_))));
}

#[tokio::test]
async fn get_parses_json_object() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::GET).path("/get/f1");
        then.status(200).json_body(serde_json::json!({
            "Leaf": {"name": "file", "id": "f1", "meta": {"size": null, "content_type": null, "modified": null, "extra": {}}}
        }));
    });
    let origin = make_origin(&server);

    let object = origin.get(&ObjectId::from("f1")).await.unwrap();
    assert_eq!(object.get_name(), "file");
    assert_eq!(object.get_id().as_str(), "f1");
}

#[tokio::test]
async fn get_returns_not_found_on_404() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::GET).path("/get/missing");
        then.status(404);
    });
    let origin = make_origin(&server);

    let result = origin.get(&ObjectId::from("missing")).await;
    assert!(matches!(result, Err(VaultError::NotFound(_))));
}

#[tokio::test]
async fn get_returns_generic_error_on_invalid_json() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::GET).path("/get/f1");
        then.status(200).body("not json");
    });
    let origin = make_origin(&server);

    let result = origin.get(&ObjectId::from("f1")).await;
    assert!(matches!(result, Err(VaultError::HTTP(_))));
}

#[tokio::test]
async fn list_parses_json_array() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::GET).path("/list/dir");
        then.status(200).json_body(serde_json::json!([
            {"Leaf": {"name": "a", "id": "a", "meta": {"size": null, "content_type": null, "modified": null, "extra": {}}}}
        ]));
    });
    let origin = make_origin(&server);

    let objects = origin.list(&ObjectId::from("dir")).await.unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].get_name(), "a");
}

#[tokio::test]
async fn put_sends_object_as_json_body_to_destination_url() {
    let server = MockServer::start();
    let put_mock = server.mock(|when, then| {
        when.method(Method::PUT)
            .path("/put/dir")
            .json_body(serde_json::json!({
                "Leaf": {"name": "file.txt", "id": "f1", "meta": {"size": null, "content_type": null, "modified": null, "extra": {}}}
            }));
        then.status(201);
    });
    // put's returned Object comes from a follow-up get on "{destination}/{name}"
    let get_mock = server.mock(|when, then| {
        when.method(Method::GET).path("/get/dir/file.txt");
        then.status(200).json_body(serde_json::json!({
            "Leaf": {"name": "file.txt", "id": "dir/file.txt", "meta": {"size": null, "content_type": null, "modified": null, "extra": {}}}
        }));
    });
    let origin = make_origin(&server);
    let mut object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("f1"),
        meta: Metadata::new(),
    };
    let result = origin
        .put(&mut object, &ObjectId::from("dir"))
        .await
        .unwrap();
    assert_eq!(result.get_id().as_str(), "dir/file.txt");
    put_mock.assert();
    get_mock.assert();
}

#[tokio::test]
async fn put_errors_on_failure_status() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::PUT).path("/put/dir");
        then.status(500);
    });
    let origin = make_origin(&server);
    let mut object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("f1"),
        meta: Metadata::new(),
    };
    let result = origin.put(&mut object, &ObjectId::from("dir")).await;
    assert!(matches!(result, Err(VaultError::OriginError(_))));
}

#[tokio::test]
async fn put_propagates_not_found_from_follow_up_get() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::PUT).path("/put/dir");
        then.status(201);
    });
    server.mock(|when, then| {
        when.method(Method::GET).path("/get/dir/file.txt");
        then.status(404);
    });
    let origin = make_origin(&server);
    let mut object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("f1"),
        meta: Metadata::new(),
    };
    let result = origin.put(&mut object, &ObjectId::from("dir")).await;
    assert!(matches!(result, Err(VaultError::NotFound(_))));
}

#[tokio::test]
async fn send_streams_payload_as_request_body() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(Method::PUT)
            .path("/send/f1")
            .body("hello world");
        then.status(200);
    });
    let origin = make_origin(&server);
    let object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("f1"),
        meta: Metadata::new(),
    };
    let payload: ByteStream = Box::pin(stream::iter(vec![
        Ok(Bytes::from_static(b"hello ")),
        Ok(Bytes::from_static(b"world")),
    ]));

    origin.send(&object, payload).await.unwrap();
    mock.assert();
}

#[tokio::test]
async fn delete_succeeds_on_2xx() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::DELETE).path("/delete/f1");
        then.status(204);
    });
    let origin = make_origin(&server);
    origin.delete(&ObjectId::from("f1")).await.unwrap();
}

#[tokio::test]
async fn delete_returns_not_found_on_404() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::DELETE).path("/delete/missing");
        then.status(404);
    });
    let origin = make_origin(&server);
    let result = origin.delete(&ObjectId::from("missing")).await;
    assert!(matches!(result, Err(VaultError::NotFound(_))));
}

// --- authentication ---

/// The header `auth` resolves to, as plain text for comparing. `Secret` has no `==` on purpose.
fn header(auth: &HttpAuth) -> Option<String> {
    auth.header_value()
        .unwrap()
        .map(|value| value.expose().to_string())
}

fn bearer(token: &str) -> HttpAuth {
    HttpAuth::Bearer {
        token: Some(token.into()),
        token_env: None,
        token_file: None,
    }
}

#[tokio::test]
async fn every_operation_carries_the_configured_credentials() {
    let server = MockServer::start();
    // One mock per operation, each demanding the header — an operation that forgot to
    // authenticate would miss its mock and fail the request instead.
    let mocks: Vec<_> = [
        (Method::GET, "/list/f1"),
        (Method::GET, "/get/f1"),
        (Method::GET, "/fetch/f1"),
        (Method::PUT, "/put/f1"),
        (Method::DELETE, "/delete/f1"),
    ]
    .into_iter()
    .map(|(method, path)| {
        server.mock(|when, then| {
            when.method(method)
                .path(path)
                .header("authorization", "Bearer s3cr3t");
            then.status(200).json_body(serde_json::json!([]));
        })
    })
    .collect();

    let origin = make_origin(&server).with_auth(&bearer("s3cr3t")).unwrap();
    let id = ObjectId::from("f1");

    origin.list(&id).await.unwrap();
    let _ = origin.get(&id).await; // the empty-array body isn't an Object; the request is what matters
    let _stream = origin.fetch(&id).await.unwrap();
    let _ = origin.put(&mut leaf("f1"), &id).await;
    origin.delete(&id).await.unwrap();

    for mock in mocks {
        mock.assert();
    }
}

fn leaf(name: &str) -> Object {
    Object::Leaf {
        name: name.to_string(),
        id: ObjectId::from(name),
        meta: Metadata::new(),
    }
}

#[tokio::test]
async fn no_authorization_header_is_sent_without_credentials() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(Method::GET)
            .path("/list/f1")
            .matches(|request| {
                request.headers.as_ref().is_none_or(|headers| {
                    !headers
                        .iter()
                        .any(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                })
            });
        then.status(200).json_body(serde_json::json!([]));
    });

    make_origin(&server)
        .list(&ObjectId::from("f1"))
        .await
        .unwrap();

    mock.assert();
}

#[tokio::test]
async fn a_401_says_to_look_at_the_auth_config() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::GET).path("/list/f1");
        then.status(401);
    });

    let error = make_origin(&server)
        .list(&ObjectId::from("f1"))
        .await
        .unwrap_err();

    let message = error.to_string();
    assert!(message.contains("401"), "{message}");
    assert!(message.contains("origin_config.auth"), "{message}");
}

#[test]
fn no_credentials_means_no_header_value() {
    assert_eq!(header(&HttpAuth::None), None);
}

#[test]
fn a_literal_token_becomes_a_bearer_header() {
    assert_eq!(header(&bearer("s3cr3t")), Some("Bearer s3cr3t".to_string()));
}

#[test]
fn a_token_can_come_from_a_file_without_its_trailing_newline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("token");
    std::fs::write(&path, "s3cr3t\n").unwrap();

    let auth = HttpAuth::Bearer {
        token: None,
        token_env: None,
        token_file: Some(path),
    };

    assert_eq!(header(&auth), Some("Bearer s3cr3t".to_string()));
}

#[test]
fn a_missing_token_file_is_reported_with_its_path() {
    let auth = HttpAuth::Bearer {
        token: None,
        token_env: None,
        token_file: Some("/nonexistent/token".into()),
    };

    let error = auth.header_value().unwrap_err().to_string();
    assert!(error.contains("/nonexistent/token"), "{error}");
}

#[test]
fn a_token_can_come_from_the_environment() {
    // A name unique to this test: the environment is process-wide and tests share it.
    let name = "NIMBUS_TEST_TOKEN_FROM_ENV";
    unsafe { std::env::set_var(name, "s3cr3t") };

    let auth = HttpAuth::Bearer {
        token: None,
        token_env: Some(name.to_string()),
        token_file: None,
    };

    assert_eq!(header(&auth), Some("Bearer s3cr3t".to_string()));
    unsafe { std::env::remove_var(name) };
}

#[test]
fn an_unset_environment_variable_is_reported_by_name() {
    let auth = HttpAuth::Bearer {
        token: None,
        token_env: Some("NIMBUS_TEST_TOKEN_NEVER_SET".to_string()),
        token_file: None,
    };

    let error = auth.header_value().unwrap_err().to_string();
    assert!(error.contains("NIMBUS_TEST_TOKEN_NEVER_SET"), "{error}");
}

#[test]
fn a_bearer_with_no_source_at_all_is_an_error() {
    let auth = HttpAuth::Bearer {
        token: None,
        token_env: None,
        token_file: None,
    };

    let error = auth.header_value().unwrap_err().to_string();
    assert!(error.contains("set one of"), "{error}");
}

#[test]
fn two_token_sources_at_once_are_refused_rather_than_ranked() {
    // Silently preferring one would let a stale `token` override the `token_env` someone
    // added to replace it.
    let auth = HttpAuth::Bearer {
        token: Some("from-config".into()),
        token_env: Some("NIMBUS_TEST_TOKEN_UNUSED".to_string()),
        token_file: None,
    };

    let error = auth.header_value().unwrap_err().to_string();
    assert!(error.contains("only one"), "{error}");
}

#[test]
fn an_empty_token_is_an_error_rather_than_an_empty_header() {
    let auth = bearer("");
    let error = auth.header_value().unwrap_err().to_string();
    assert!(error.contains("empty"), "{error}");
}

#[test]
fn a_token_that_cannot_be_a_header_is_refused_when_the_origin_is_built() {
    let result = OriginHTTP::new(
        "http://x".to_string(),
        "/fetch/{id}".to_string(),
        "/list/{id}".to_string(),
        "/get/{id}".to_string(),
        "/put/{id}".to_string(),
        "/send/{id}".to_string(),
        "/delete/{id}".to_string(),
    )
    .with_auth(&bearer("line\nbreak"));

    assert!(result.is_err());
}

// --- regressions: ids are percent-encoded, names are checked ---

#[test]
fn url_percent_encodes_characters_that_would_change_the_url() {
    let origin = OriginHTTP::new(
        "http://x".to_string(),
        "/fetch/{id}".to_string(),
        "/list/{id}".to_string(),
        "/get/{id}".to_string(),
        "/put/{id}".to_string(),
        "/send/{id}".to_string(),
        "/delete/{id}".to_string(),
    );
    let url = |id: &str| origin.url(&origin.send_url, &ObjectId::from(id));
    // `#` used to start a fragment, so the server saw `/send/a` and overwrote `a`.
    assert_eq!(url("a#b"), "http://x/send/a%23b");
    assert_eq!(url("a?b"), "http://x/send/a%3Fb");
    assert_eq!(url("50%"), "http://x/send/50%25");
    assert_eq!(url("sp ace"), "http://x/send/sp%20ace");
    assert_eq!(url("ñ"), "http://x/send/%C3%B1");
    // `/` still separates segments; plain ids are unchanged.
    assert_eq!(url("docs/a#b.txt"), "http://x/send/docs/a%23b.txt");
    assert_eq!(url("/docs/notes.txt"), "http://x/send//docs/notes.txt");
}

#[tokio::test]
async fn put_into_root_slash_does_not_produce_a_double_slash_id() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::PUT).path("/put//");
        then.status(200);
    });
    let get = server.mock(|when, then| {
        when.method(Method::GET).path("/get//file.txt");
        then.status(200).json_body(serde_json::json!({
            "Leaf": {"name": "file.txt", "id": "/file.txt",
                     "meta": {"size": null, "content_type": null, "modified": null, "extra": {}}}
        }));
    });
    let origin = make_origin(&server);
    let mut object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("file.txt"),
        meta: Metadata::new(),
    };

    origin.put(&mut object, &ObjectId::from("/")).await.unwrap();
    // `/` + `file.txt` → `/file.txt`, not `//file.txt` (which encodes to `/get///file.txt`).
    get.assert();
}

#[tokio::test]
async fn put_rejects_names_that_are_not_one_path_component() {
    let server = MockServer::start();
    let put = server.mock(|when, then| {
        when.method(Method::PUT);
        then.status(200);
    });
    let origin = make_origin(&server);
    for name in ["../x", "a/b", ".."] {
        let mut object = Object::Leaf {
            name: name.to_string(),
            id: ObjectId::from("x"),
            meta: Metadata::new(),
        };
        let result = origin.put(&mut object, &ObjectId::from("")).await;
        assert!(
            matches!(result, Err(VaultError::InvalidName(_))),
            "{name:?}"
        );
    }
    // Rejected before anything was sent.
    put.assert_hits(0);
}

#[tokio::test]
async fn rename_without_a_rename_url_is_unsupported() {
    let server = MockServer::start();
    let origin = make_origin(&server);
    let result = origin
        .rename(&ObjectId::from("a"), &ObjectId::from(""), "b")
        .await;
    assert!(matches!(result, Err(VaultError::Unsupported(_))));
}

#[tokio::test]
async fn a_501_answer_to_rename_is_unsupported_so_callers_fall_back() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::POST).path("/rename/a");
        then.status(501);
    });
    let origin = make_origin(&server)
        .with_rename_url(Some("/rename/{id}".to_string()))
        .unwrap();
    let result = origin
        .rename(&ObjectId::from("a"), &ObjectId::from(""), "b")
        .await;
    assert!(
        matches!(result, Err(VaultError::Unsupported(_))),
        "{result:?}"
    );
}

// --- regressions: a stalled server fails the request instead of hanging forever ---

/// A server that accepts connections and then never answers.
async fn stalled_server() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket); // keep it open, read nothing, write nothing
        }
    });
    format!("http://{address}")
}

fn origin_at(base: String) -> OriginHTTP {
    OriginHTTP::new(
        base,
        "/fetch/{id}".to_string(),
        "/list/{id}".to_string(),
        "/get/{id}".to_string(),
        "/put/{id}".to_string(),
        "/send/{id}".to_string(),
        "/delete/{id}".to_string(),
    )
}

#[tokio::test]
async fn a_request_to_a_stalled_server_times_out() {
    let origin = origin_at(stalled_server().await)
        .with_timeouts(
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(300),
        )
        .unwrap();
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        origin.list(&ObjectId::from("")),
    )
    .await
    .expect("the request hung instead of timing out");
    assert!(matches!(result, Err(VaultError::HTTP(_))), "{result:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[tokio::test]
async fn the_read_timeout_is_idle_based_so_a_slow_but_moving_body_completes() {
    // A body that trickles in for longer than the read timeout, but never pauses that long.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 1024];
        let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut request).await;
        socket
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 10\r\n\r\n")
            .await
            .unwrap();
        for _ in 0..10 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            socket.write_all(b"x").await.unwrap();
        }
    });
    let origin = origin_at(format!("http://{address}"))
        .with_timeouts(
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(400),
        )
        .unwrap();

    let mut stream = origin.fetch(&ObjectId::from("big")).await.unwrap();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        body.extend_from_slice(&chunk.unwrap());
    }
    // ~1 s in total, well past the 400 ms read timeout.
    assert_eq!(body, b"xxxxxxxxxx");
}

#[tokio::test]
async fn an_unreachable_server_fails_fast() {
    // Bind then drop, so nothing listens on that port.
    let address = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };
    let origin = origin_at(format!("http://{address}"));
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        origin.get(&ObjectId::from("a")),
    )
    .await
    .expect("connecting hung");
    assert!(result.is_err());
}

#[test]
fn no_debug_view_shows_the_token() {
    let token = "s3cr3t-in-debug";
    let auth = bearer(token);
    let header = auth.header_value().unwrap();

    // A whole vault opened from a config holding the token inline.
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("vault.toml");
    std::fs::write(
        &config,
        format!(
            "name = \"remote\"\n\n[origin_config]\ntype = \"http\"\n\
             base_url = \"http://127.0.0.1:9\"\nlist_url = \"/list/{{id}}\"\n\
             fetch_url = \"/fetch/{{id}}\"\nget_url = \"/get/{{id}}\"\nput_url = \"/put/{{id}}\"\n\
             send_url = \"/send/{{id}}\"\ndelete_url = \"/delete/{{id}}\"\n\n\
             [origin_config.auth]\ntype = \"bearer\"\ntoken = \"{token}\"\n"
        ),
    )
    .unwrap();
    let vault = crate::vault::Vault::new(config).unwrap();

    // Errors about a configured token, which carry it nowhere either.
    let unsendable = bearer(&format!("{token}\n"));
    let Err(unsendable) = origin_with_auth(&unsendable) else {
        panic!("a token with a newline can't be sent in a header");
    };
    let ambiguous = HttpAuth::Bearer {
        token: Some(token.into()),
        token_env: Some("NIMBUS_TEST_TOKEN_UNUSED".to_string()),
        token_file: None,
    }
    .header_value()
    .unwrap_err();

    for (what, debug) in [
        ("HttpAuth", format!("{auth:?}")),
        ("header_value", format!("{header:?}")),
        ("Vault", format!("{vault:?}")),
        ("unsendable-token error", format!("{unsendable:?}")),
        ("two-sources error", format!("{ambiguous:?}")),
    ] {
        assert!(!debug.contains(token), "{what} shows the token: {debug}");
    }
}

/// An `OriginHTTP` with `auth` applied, for the errors `with_auth` reports.
fn origin_with_auth(auth: &HttpAuth) -> VaultResult<OriginHTTP> {
    OriginHTTP::new(
        "http://127.0.0.1:9".to_string(),
        "/fetch/{id}".to_string(),
        "/list/{id}".to_string(),
        "/get/{id}".to_string(),
        "/put/{id}".to_string(),
        "/send/{id}".to_string(),
        "/delete/{id}".to_string(),
    )
    .with_auth(auth)
}
