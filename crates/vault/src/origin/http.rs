use futures::StreamExt;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};

use crate::{
    VaultResult,
    error::VaultError,
    object::{Object, ObjectId},
    origin::{ByteStream, Origin},
};

/// The credentials an [`OriginHTTP`] presents on every request — the client-side counterpart
/// of what a server (e.g. `nimbus-daemon`) checks.
///
/// Declared inside a vault's `[origin_config]`, and tagged by `type` so a new scheme is an
/// added variant rather than a breaking change to existing configs:
///
/// ```toml
/// [origin_config.auth]
/// type = "bearer"
/// token_env = "NIMBUS_TOKEN"
/// ```
///
/// The secret comes from exactly one of three places: the config itself (`token`), an
/// environment variable (`token_env`), or a file (`token_file`). `token_env` is the one to
/// reach for — a vault config is a file people copy between machines and commit, which is no
/// place for a password.
///
/// # Examples
///
/// ```
/// use nimbus_vault::origin::http::HttpAuth;
///
/// // No credentials is the default, and sends no header at all.
/// assert_eq!(HttpAuth::default().header_value()?, None);
///
/// let auth = HttpAuth::Bearer {
///     token: Some("s3cr3t".to_string()),
///     token_env: None,
///     token_file: None,
/// };
/// assert_eq!(auth.header_value()?, Some("Bearer s3cr3t".to_string()));
/// # Ok::<(), nimbus_vault::error::VaultError>(())
/// ```
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HttpAuth {
    /// Send nothing. The default, for an origin that doesn't need credentials.
    #[default]
    None,
    /// A shared secret sent as `Authorization: Bearer <token>`.
    Bearer {
        /// The token, written out in the config.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
        /// Name of an environment variable holding the token.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token_env: Option<String>,
        /// Path to a file holding the token; a trailing newline is not part of it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token_file: Option<PathBuf>,
    },
}

impl HttpAuth {
    /// Resolves the secret and returns the `Authorization` header value to send, or `None`
    /// when no credentials are configured.
    pub fn header_value(&self) -> VaultResult<Option<String>> {
        match self {
            HttpAuth::None => Ok(None),
            HttpAuth::Bearer {
                token,
                token_env,
                token_file,
            } => {
                let secret = resolve_secret(
                    token.as_deref(),
                    token_env.as_deref(),
                    token_file.as_deref(),
                )?;
                Ok(Some(format!("Bearer {secret}")))
            }
        }
    }
}

/// Reads the secret from whichever of the three sources was configured, insisting that it be
/// exactly one — silently preferring one over another would make a stale `token` in a config
/// override the `token_env` someone added to replace it.
fn resolve_secret(
    literal: Option<&str>,
    variable: Option<&str>,
    file: Option<&Path>,
) -> VaultResult<String> {
    let secret = match (literal, variable, file) {
        (Some(token), None, None) => token.to_string(),
        (None, Some(name), None) => std::env::var(name).map_err(|_| {
            VaultError::Generic(format!("auth: environment variable '{name}' is not set"))
        })?,
        (None, None, Some(path)) => {
            let raw = std::fs::read_to_string(path).map_err(|e| {
                VaultError::Generic(format!("auth: reading {}: {e}", path.display()))
            })?;
            // A secret written by an editor or `echo` ends in a newline that isn't part of it.
            raw.trim_end_matches(['\r', '\n']).to_string()
        }
        (None, None, None) => {
            return Err(VaultError::Generic(
                "auth: set one of token, token_env or token_file".to_string(),
            ));
        }
        _ => {
            return Err(VaultError::Generic(
                "auth: set only one of token, token_env or token_file".to_string(),
            ));
        }
    };

    if secret.is_empty() {
        return Err(VaultError::Generic(
            "auth: the configured token is empty".to_string(),
        ));
    }
    Ok(secret)
}

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
///
/// # optional; omitted means no credentials are sent — see `HttpAuth`
/// [origin_config.auth]
/// type = "bearer"
/// token_env = "NIMBUS_TOKEN"
/// ```
pub struct OriginHTTP {
    base_url: String,

    fetch_url: String,
    list_url: String,
    get_url: String,

    put_url: String,
    send_url: String,
    delete_url: String,

    /// Sent on every request; `None` when the origin has no credentials configured.
    auth: Option<reqwest::header::HeaderValue>,
    client: reqwest::Client,
}

impl OriginHTTP {
    /// Builds an `OriginHTTP` from `base_url` plus one `{id}`-templated path per operation,
    /// with no credentials — see [`OriginHTTP::with_auth`].
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
            auth: None,
            client: reqwest::Client::new(),
        }
    }

    /// Presents `auth` on every request this origin makes.
    ///
    /// The secret is resolved here rather than per request, so a missing environment variable
    /// or an unreadable token file surfaces when the vault is opened — not as a 401 halfway
    /// through a sync.
    ///
    /// # Examples
    ///
    /// ```
    /// use nimbus_vault::origin::http::{HttpAuth, OriginHTTP};
    ///
    /// let origin = OriginHTTP::new(
    ///     "http://server:8080/v/photos".to_string(),
    ///     "/fetch/{id}".to_string(),
    ///     "/list/{id}".to_string(),
    ///     "/get/{id}".to_string(),
    ///     "/put/{id}".to_string(),
    ///     "/send/{id}".to_string(),
    ///     "/delete/{id}".to_string(),
    /// )
    /// .with_auth(&HttpAuth::Bearer {
    ///     token: Some("s3cr3t".to_string()),
    ///     token_env: None,
    ///     token_file: None,
    /// })?;
    /// # let _ = origin;
    /// # Ok::<(), nimbus_vault::error::VaultError>(())
    /// ```
    pub fn with_auth(mut self, auth: &HttpAuth) -> VaultResult<Self> {
        self.auth = match auth.header_value()? {
            Some(value) => {
                let mut header = reqwest::header::HeaderValue::from_str(&value).map_err(|_| {
                    VaultError::Generic(
                        "auth: the token contains characters that can't be sent in a header"
                            .to_string(),
                    )
                })?;
                // Keeps the secret out of `{:?}` output, reqwest's logging included.
                header.set_sensitive(true);
                Some(header)
            }
            None => None,
        };
        Ok(self)
    }

    fn url(&self, template: &str, id: &ObjectId) -> String {
        format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            template.replace(&format!("{{{}}}", crate::PLACEHOLDER_ID), id.as_str())
        )
    }

    /// Sends a request and turns non-2xx responses into `VaultError`, mapping 404 to `NotFound`.
    ///
    /// Every request in this file goes through here, which is why it's also where the
    /// credentials are attached: an operation added later can't forget to authenticate.
    async fn execute(
        &self,
        request: reqwest::RequestBuilder,
        url: &str,
    ) -> VaultResult<reqwest::Response> {
        let request = match &self.auth {
            Some(header) => request.header(reqwest::header::AUTHORIZATION, header),
            None => request,
        };
        let response = request.send().await?;

        match response.status() {
            s if s.is_success() => Ok(response),
            reqwest::StatusCode::NOT_FOUND => Err(VaultError::NotFound(url.to_string())),
            // The one status whose cause is in the config rather than at the other end: say so
            // instead of leaving a bare "401" for the user to interpret.
            s @ (reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN) => {
                Err(VaultError::OriginError(format!(
                    "request to {url} was refused with status {s} — check this vault's \
                     [origin_config.auth]"
                )))
            }
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
