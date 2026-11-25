use futures::StreamExt;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};

use crate::secret::Secret;

pub use crate::origin::tls::HttpTls;
use crate::origin::tls::{ServerTrust, require_https};
use crate::{
    VaultResult,
    error::VaultError,
    object::{Object, ObjectId, validate_name},
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
/// assert!(HttpAuth::default().header_value()?.is_none());
///
/// let auth = HttpAuth::Bearer {
///     token: Some("s3cr3t".into()),
///     token_env: None,
///     token_file: None,
/// };
/// let header = auth.header_value()?.unwrap();
/// assert_eq!(header.expose(), "Bearer s3cr3t");
/// // Neither the config nor the header shows the token in `{:?}`.
/// assert!(!format!("{auth:?} {header:?}").contains("s3cr3t"));
/// # Ok::<(), nimbus_vault::error::VaultError>(())
/// ```
///
/// `Debug` never shows an inline token (it is a [`Secret`]). There is deliberately no `==`,
/// since comparing would mean comparing secrets.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HttpAuth {
    /// Send nothing. The default, for an origin that doesn't need credentials.
    #[default]
    None,
    /// A shared secret sent as `Authorization: Bearer <token>`.
    Bearer {
        /// The token, written out in the config. Read it with [`Secret::expose`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<Secret>,
        /// Name of an environment variable holding the token.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token_env: Option<String>,
        /// Path to a file holding the token; a trailing newline is not part of it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token_file: Option<PathBuf>,
    },
}

impl HttpAuth {
    /// Resolves a relative `token_file` against `dir`, the directory of the config file it came
    /// from (see [`crate::config::OriginConfig::relative_to`]). Inline and environment-variable
    /// tokens have no path, so they are untouched.
    pub(crate) fn relative_to(self, dir: &Path) -> Self {
        match self {
            HttpAuth::Bearer {
                token,
                token_env,
                token_file,
            } => HttpAuth::Bearer {
                token,
                token_env,
                token_file: token_file.map(|path| crate::fsutil::resolve_against(dir, path)),
            },
            none => none,
        }
    }

    /// Resolves the secret and returns the `Authorization` header value to send, or `None`
    /// when no credentials are configured. The header carries the token, so it is a
    /// [`Secret`] too.
    pub fn header_value(&self) -> VaultResult<Option<Secret>> {
        match self {
            HttpAuth::None => Ok(None),
            HttpAuth::Bearer {
                token,
                token_env,
                token_file,
            } => {
                let secret =
                    resolve_secret(token.as_ref(), token_env.as_deref(), token_file.as_deref())?;
                Ok(Some(format!("Bearer {}", secret.expose()).into()))
            }
        }
    }
}

/// Reads the secret from whichever of the three sources was configured, insisting that it be
/// exactly one — silently preferring one over another would make a stale `token` in a config
/// override the `token_env` someone added to replace it.
fn resolve_secret(
    literal: Option<&Secret>,
    variable: Option<&str>,
    file: Option<&Path>,
) -> VaultResult<Secret> {
    let secret = match (literal, variable, file) {
        (Some(token), None, None) => token.clone(),
        (None, Some(name), None) => std::env::var(name)
            .map_err(|_| {
                VaultError::Generic(format!("auth: environment variable '{name}' is not set"))
            })?
            .into(),
        (None, None, Some(path)) => crate::fsutil::read_secret(path)
            .map_err(|e| VaultError::Generic(format!("auth: reading {}: {e}", path.display())))?
            .into(),
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

/// How long to wait for a connection to the server before giving up.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// How long a request may go without receiving *any* bytes. An idle timeout, not a total one,
/// so a large transfer that keeps moving is never cut off. A stalled server, though, no longer
/// hangs the CLI/TUI forever.
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Everything the HTTP client is built from. Kept as one value so every `with_*` builder
/// rebuilds the client from all of it: setting timeouts after TLS (or the reverse) can't drop
/// the other.
#[derive(Clone)]
struct ClientSettings {
    connect_timeout: std::time::Duration,
    read_timeout: std::time::Duration,
    trust: Option<ServerTrust>,
}

impl Default for ClientSettings {
    fn default() -> Self {
        ClientSettings {
            connect_timeout: CONNECT_TIMEOUT,
            read_timeout: READ_TIMEOUT,
            trust: None,
        }
    }
}

fn build_client(settings: &ClientSettings) -> VaultResult<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(settings.connect_timeout)
        .read_timeout(settings.read_timeout);
    match &settings.trust {
        None => {}
        Some(ServerTrust::Ca(certificates)) => {
            builder = builder.tls_built_in_root_certs(false);
            for certificate in certificates {
                builder = builder.add_root_certificate(certificate.clone());
            }
        }
        Some(ServerTrust::Pin(pin)) => {
            builder =
                builder.use_preconfigured_tls(crate::origin::tls::pinned_client_config(*pin)?);
        }
    }
    Ok(builder.build()?)
}

/// Everything that can't appear raw inside one URL path segment. `/` is deliberately absent:
/// it separates the id's own segments, which the server splits on.
const PATH_SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// Percent-encodes `id` for use in a URL path. Without this a name like `a#b` was cut at the
/// `#` (the rest becomes a fragment the server never sees), so a `send` meant for `a#b`
/// overwrote the unrelated object `a`. `?` and `%` broke the same way.
fn encode_id(id: &ObjectId) -> String {
    percent_encoding::utf8_percent_encode(id.as_str(), PATH_SEGMENT).to_string()
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
    /// Optional `{id}`-templated path for a native move; see [`OriginHTTP::with_rename_url`].
    rename_url: Option<String>,

    /// Sent on every request; `None` when the origin has no credentials configured.
    auth: Option<reqwest::header::HeaderValue>,
    /// What `client` was built from; see [`ClientSettings`].
    settings: ClientSettings,
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
            rename_url: None,
            auth: None,
            settings: ClientSettings::default(),
            // Only fails if the TLS backend can't initialise, which `Client::new()` would hit too.
            client: build_client(&ClientSettings::default())
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    /// Replaces the default timeouts: `connect` (10 s) to establish a connection, and `read`
    /// (60 s), how long a request may go without receiving any bytes. `read` is an idle
    /// timeout, so a transfer that keeps moving is never cut off.
    pub fn with_timeouts(
        mut self,
        connect: std::time::Duration,
        read: std::time::Duration,
    ) -> VaultResult<Self> {
        self.settings.connect_timeout = connect;
        self.settings.read_timeout = read;
        self.client = build_client(&self.settings)?;
        Ok(self)
    }

    /// Applies `[origin_config.tls]` (see [`HttpTls`]). Checked here, when the vault opens,
    /// rather than on the first request; see [`HttpTls`] for what is refused.
    pub fn with_tls(mut self, tls: &HttpTls) -> VaultResult<Self> {
        let trust = tls.resolve(self.request_urls().iter().map(String::as_str))?;
        self.settings.trust = Some(trust);
        self.client = build_client(&self.settings)?;
        Ok(self)
    }

    /// Enables [`Origin::rename`]: `POST <rename_url>` with a JSON body
    /// `{"destination": <parent id>, "name": <new name>}`, answered with the moved `Object`.
    /// `nimbus-daemon` serves this at `/rename/{id}`. Without it, or when the server answers
    /// `501 Not Implemented`, moves fall back to copy-then-delete.
    ///
    /// When TLS settings are already applied (see [`OriginHTTP::with_tls`]), the new URL must be
    /// `https://` too, whichever order the two are set in.
    pub fn with_rename_url(mut self, rename_url: Option<String>) -> VaultResult<Self> {
        self.rename_url = rename_url;
        if self.settings.trust.is_some() {
            require_https(self.request_urls().iter().map(String::as_str))?;
        }
        Ok(self)
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
    ///     token: Some("s3cr3t".into()),
    ///     token_env: None,
    ///     token_file: None,
    /// })?;
    /// # let _ = origin;
    /// # Ok::<(), nimbus_vault::error::VaultError>(())
    /// ```
    pub fn with_auth(mut self, auth: &HttpAuth) -> VaultResult<Self> {
        self.auth = match auth.header_value()? {
            Some(value) => {
                let mut header =
                    reqwest::header::HeaderValue::from_str(value.expose()).map_err(|_| {
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
            template.replace(&format!("{{{}}}", crate::PLACEHOLDER_ID), &encode_id(id))
        )
    }

    /// Every URL this origin can request, with an empty id: one per operation template, plus
    /// `rename_url` when set. A template may be a full URL that ignores `base_url`, so this is
    /// what [`HttpTls::resolve`] checks rather than `base_url` alone.
    fn request_urls(&self) -> Vec<String> {
        [
            &self.fetch_url,
            &self.list_url,
            &self.get_url,
            &self.put_url,
            &self.send_url,
            &self.delete_url,
        ]
        .into_iter()
        .chain(&self.rename_url)
        .map(|template| self.url(template, &ObjectId::from("")))
        .collect()
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
            reqwest::StatusCode::NOT_IMPLEMENTED => Err(VaultError::Unsupported(format!(
                "the server at {url} doesn't support this operation"
            ))),
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
        validate_name(&object.get_name())?;
        let url = self.url(&self.put_url, destination);
        self.execute(self.client.put(&url).json(object), &url)
            .await?;
        let new_id = ObjectId::from(format!(
            "{}/{}",
            destination.as_str().trim_end_matches('/'),
            object.get_name()
        ));
        self.get(&new_id).await
    }

    async fn send(&self, object: &Object, payload: ByteStream) -> VaultResult<()> {
        let url = self.url(&self.send_url, &object.get_id());
        let body = reqwest::Body::wrap_stream(payload);
        let mut request = self.client.put(&url).body(body);
        if let Some(modified) = object.get_meta().and_then(|meta| meta.modified) {
            request = request.header(
                crate::MODIFIED_HEADER,
                modified.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
            );
        }
        self.execute(request, &url).await?;
        Ok(())
    }

    async fn rename(
        &self,
        id: &ObjectId,
        new_parent: &ObjectId,
        new_name: &str,
    ) -> VaultResult<Object> {
        let Some(template) = &self.rename_url else {
            return Err(VaultError::Unsupported(
                "rename (no rename_url configured)".into(),
            ));
        };
        validate_name(new_name)?;
        let url = self.url(template, id);
        let body = serde_json::json!({ "destination": new_parent.as_str(), "name": new_name });
        let response = self
            .execute(self.client.post(&url).json(&body), &url)
            .await?;
        Ok(response.json().await?)
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
