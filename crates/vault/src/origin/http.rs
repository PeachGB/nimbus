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
            template.replace(&format!("{{{}}}", crate::PLACEHOLDER_ID), id.as_str())
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

    async fn put(&self, object: &mut Object, destination: &ObjectId) -> VaultResult<Object> {
        let url = self.url(&self.put_url, destination);
        self.execute(self.client.put(&url).json(object), &url)
            .await?;
        let new_id = ObjectId::from(format!("{}/{}", destination.as_str(), object.get_name()));
        self.get(&new_id).await
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
#[path = "tests/http.rs"]
mod tests;
