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

fn bearer(token: &str) -> HttpAuth {
    HttpAuth::Bearer {
        token: Some(token.to_string()),
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
    assert_eq!(HttpAuth::None.header_value().unwrap(), None);
}

#[test]
fn a_literal_token_becomes_a_bearer_header() {
    assert_eq!(
        bearer("s3cr3t").header_value().unwrap(),
        Some("Bearer s3cr3t".to_string())
    );
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

    assert_eq!(
        auth.header_value().unwrap(),
        Some("Bearer s3cr3t".to_string())
    );
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

    assert_eq!(
        auth.header_value().unwrap(),
        Some("Bearer s3cr3t".to_string())
    );
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
        token: Some("from-config".to_string()),
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
