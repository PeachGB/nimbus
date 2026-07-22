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
