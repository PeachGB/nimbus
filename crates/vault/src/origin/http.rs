use futures::StreamExt;
use serde::de::DeserializeOwned;

use crate::{
    VaultResult,
    error::VaultError,
    object::{Object, ObjectId},
    origin::{ByteStream, Origin},
};

/// An [`crate::origin::Origin`] backed by a REST-ish HTTP API. Each operation is a
/// `{id}`-templated path appended to `base_url`; `get`/`list` are `GET`s deserialized as
/// JSON, `fetch` streams the response body, `put` `PUT`s the `Object` as a JSON body, `send`
/// `PUT`s the payload stream as the request body, and `delete` is a `DELETE`. Any non-2xx
/// response becomes a `VaultError`, with 404 mapped to `NotFound`.
///
/// # Examples
///
/// ```
/// use httpmock::MockServer; // test-only mock server, shown here to keep the example runnable
/// use nimbus_vault::{object::ObjectId, origin::{Origin, http::OriginHTTP}};
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let server = MockServer::start();
/// server.mock(|when, then| {
///     when.method(httpmock::Method::GET).path("/get/notes.txt");
///     then.status(200).json_body(serde_json::json!({
///         "Leaf": {
///             "name": "notes.txt",
///             "id": "notes.txt",
///             "meta": { "size": 5, "content_type": null, "modified": null, "extra": {} },
///         }
///     }));
/// });
///
/// let origin = OriginHTTP::new(
///     server.base_url(),
///     "/fetch/{id}".to_string(),
///     "/list/{id}".to_string(),
///     "/get/{id}".to_string(),
///     "/put/{id}".to_string(),
///     "/send/{id}".to_string(),
///     "/delete/{id}".to_string(),
/// );
///
/// let object = origin.get(&ObjectId::from("notes.txt")).await?;
/// assert_eq!(object.get_name(), "notes.txt");
/// # Ok(())
/// # }
/// ```
///
/// Declaratively, via `[origin_config]` in a vault's TOML config:
///
/// ```toml
/// [origin_config]
/// type = "http"
/// base_url   = "https://example.com"
/// list_url   = "/list/{id}"
/// fetch_url  = "/fetch/{id}"
/// get_url    = "/get/{id}"
/// put_url    = "/put/{id}"
/// send_url   = "/send/{id}"
/// delete_url = "/delete/{id}"
/// ```
pub struct OriginHTTP {
    base_url: String,

    fetch_url: String,
    list_url: String,
    get_url: String,

    put_url: String,
    send_url: String,
    delete_url: String,

    client: reqwest::Client,
}

impl OriginHTTP {
    /// Builds an `OriginHTTP` from `base_url` plus one `{id}`-templated path per operation.
    pub fn new(
        base_url: String,
        fetch_url: String,
        list_url: String,
        get_url: String,
        put_url: String,
        send_url: String,
        delete_url: String,
    ) -> Self {
        OriginHTTP {
            base_url,
            fetch_url,
            list_url,
            get_url,
            put_url,
            send_url,
            delete_url,
            client: reqwest::Client::new(),
        }
    }

    fn url(&self, template: &str, id: &ObjectId) -> String {
        format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            template.replace("{id}", id.as_str())
        )
    }

    /// Sends a request and turns non-2xx responses into `VaultError`, mapping 404 to `NotFound`.
    async fn execute(
        &self,
        request: reqwest::RequestBuilder,
        url: &str,
    ) -> VaultResult<reqwest::Response> {
        let response = request.send().await?;

        match response.status() {
            s if s.is_success() => Ok(response),
            reqwest::StatusCode::NOT_FOUND => Err(VaultError::NotFound(url.to_string())),
            s => Err(VaultError::OriginError(format!(
                "request to {url} failed with status {s}"
            ))),
        }
    }

    async fn get_json<T: DeserializeOwned>(&self, template: &str, id: &ObjectId) -> VaultResult<T> {
        let url = self.url(template, id);
        let response = self.execute(self.client.get(&url), &url).await?;
        Ok(response.json().await?)
    }
}

#[async_trait::async_trait]
impl Origin for OriginHTTP {
    async fn fetch(&self, id: &ObjectId) -> VaultResult<ByteStream> {
        let url = self.url(&self.fetch_url, id);
        let response = self.execute(self.client.get(&url), &url).await?;

        let stream = response
            .bytes_stream()
            .map(|chunk| chunk.map_err(VaultError::from));
        Ok(Box::pin(stream))
    }

    async fn list(&self, id: &ObjectId) -> VaultResult<Vec<Object>> {
        self.get_json(&self.list_url, id).await
    }

    async fn get(&self, id: &ObjectId) -> VaultResult<Object> {
        self.get_json(&self.get_url, id).await
    }

    async fn put(&self, object: &Object) -> VaultResult<()> {
        let url = self.url(&self.put_url, &object.get_id());
        self.execute(self.client.put(&url).json(object), &url)
            .await?;
        Ok(())
    }

    async fn send(&self, object: &Object, payload: ByteStream) -> VaultResult<()> {
        let url = self.url(&self.send_url, &object.get_id());
        let body = reqwest::Body::wrap_stream(payload);
        self.execute(self.client.put(&url).body(body), &url).await?;
        Ok(())
    }

    async fn delete(&self, id: &ObjectId) -> VaultResult<()> {
        let url = self.url(&self.delete_url, id);
        self.execute(self.client.delete(&url), &url).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
    async fn put_sends_object_as_json_body() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(Method::PUT)
                .path("/put/f1")
                .json_body(serde_json::json!({
                    "Leaf": {"name": "file.txt", "id": "f1", "meta": {"size": null, "content_type": null, "modified": null, "extra": {}}}
                }));
            then.status(201);
        });
        let origin = make_origin(&server);
        let object = Object::Leaf {
            name: "file.txt".to_string(),
            id: ObjectId::from("f1"),
            meta: Metadata::new(),
        };
        origin.put(&object).await.unwrap();
        mock.assert();
    }

    #[tokio::test]
    async fn put_errors_on_failure_status() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(Method::PUT).path("/put/f1");
            then.status(500);
        });
        let origin = make_origin(&server);
        let object = Object::Leaf {
            name: "file.txt".to_string(),
            id: ObjectId::from("f1"),
            meta: Metadata::new(),
        };
        let result = origin.put(&object).await;
        assert!(matches!(result, Err(VaultError::OriginError(_))));
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
}
