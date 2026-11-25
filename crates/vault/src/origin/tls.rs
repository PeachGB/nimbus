//! Server trust for [`crate::origin::http::OriginHTTP`]: parsing `[origin_config.tls]`, and the
//! certificate-pinning verifier behind `pin_sha256`.
//!
//! Everything uses rustls with the **ring** provider, the same one `reqwest` is built with. A
//! second provider compiled into the same process would make rustls panic at runtime.

use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::CryptoProvider,
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use sha2::{Digest, Sha256};

use crate::{VaultResult, error::VaultError};

/// The `[origin_config.tls]` table: how to trust an `https` server that no public CA vouches
/// for. Set exactly one field.
///
/// - `ca_file`: a PEM bundle of the CA (or CAs) that signed the server's certificate. When it
///   is set, **only** these CAs are trusted; the built-in public roots are not. The hostname
///   in `base_url` must match the certificate, as usual.
/// - `pin_sha256`: the SHA-256 fingerprint of the server's own certificate. Exactly that
///   certificate is trusted, whoever signed it and whatever host name or dates it carries, the
///   way SSH trusts a host key. Get it with
///   `openssl x509 -in cert.pem -noout -fingerprint -sha256`.
///
/// There is deliberately no option to skip verification: TLS without authentication is
/// defeated by anyone in the middle.
///
/// Refused when the origin is built: any plain `http://` URL the origin would request, both
/// fields or neither, an unreadable `ca_file` or one with no certificate in it, and a
/// malformed pin.
///
/// # Examples
///
/// ```toml
/// [origin_config.tls]
/// ca_file = "/etc/nimbus/ca.pem"
/// ```
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HttpTls {
    /// PEM bundle of the CA certificate(s) to trust instead of the public roots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_file: Option<PathBuf>,
    /// SHA-256 of the server's certificate, 64 hex characters, colons allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin_sha256: Option<String>,
}

/// What the client trusts, resolved from [`HttpTls`].
#[derive(Clone)]
pub(crate) enum ServerTrust {
    /// Only these CA certificates.
    Ca(Vec<reqwest::Certificate>),
    /// Only the certificate with this SHA-256.
    Pin([u8; 32]),
}

impl HttpTls {
    /// Resolves a relative `ca_file` against `dir`, the directory of the config file it came
    /// from (see [`crate::config::OriginConfig::relative_to`]).
    pub(crate) fn relative_to(self, dir: &Path) -> Self {
        HttpTls {
            ca_file: self
                .ca_file
                .map(|path| crate::fsutil::resolve_against(dir, path)),
            ..self
        }
    }

    /// Turns the table into what the client trusts, checking it against every URL the origin
    /// will request (see [`require_https`]). Errors: a plain `http://` URL, both fields or
    /// neither, an unreadable `ca_file` or one with no certificate in it, and a malformed pin.
    pub(crate) fn resolve<'a>(
        &self,
        request_urls: impl IntoIterator<Item = &'a str>,
    ) -> VaultResult<ServerTrust> {
        require_https(request_urls)?;
        match (&self.ca_file, &self.pin_sha256) {
            (Some(_), Some(_)) => Err(invalid("set only one of ca_file or pin_sha256")),
            (None, None) => Err(invalid("set one of ca_file or pin_sha256")),
            (Some(path), None) => load_ca_file(path).map(ServerTrust::Ca),
            (None, Some(pin)) => parse_pin(pin).map(ServerTrust::Pin),
        }
    }
}

/// Errors unless every URL is `https://` (in any case). TLS settings would otherwise silently
/// not apply to some requests.
pub(crate) fn require_https<'a>(
    request_urls: impl IntoIterator<Item = &'a str>,
) -> VaultResult<()> {
    match request_urls
        .into_iter()
        .find(|url| !url.to_ascii_lowercase().starts_with("https://"))
    {
        Some(url) => Err(invalid(format!(
            "tls settings need https URLs, but {url:?} isn't one"
        ))),
        None => Ok(()),
    }
}

/// A `tls:`-prefixed configuration error, the same shape `auth:` errors take.
fn invalid(message: impl std::fmt::Display) -> VaultError {
    VaultError::Generic(format!("tls: {message}"))
}

/// Reads a PEM bundle of CA certificates, refusing a file that holds none.
fn load_ca_file(path: &Path) -> VaultResult<Vec<reqwest::Certificate>> {
    let pem = std::fs::read(path)
        .map_err(|e| invalid(format!("reading ca_file {}: {e}", path.display())))?;
    let certificates = reqwest::Certificate::from_pem_bundle(&pem)
        .map_err(|e| invalid(format!("ca_file {} isn't valid PEM: {e}", path.display())))?;
    if certificates.is_empty() {
        return Err(invalid(format!(
            "ca_file {} contains no certificate",
            path.display()
        )));
    }
    Ok(certificates)
}

/// Parses a `pin_sha256` value: the SHA-256 of the server's certificate as 64 hex characters.
/// Colons between bytes are allowed (the form `openssl x509 -fingerprint -sha256` prints) and
/// case is ignored.
///
/// # Examples
///
/// ```
/// use nimbus_vault::origin::tls::parse_pin;
///
/// let colons = "AB:".repeat(31) + "AB";
/// assert_eq!(parse_pin(&colons).unwrap(), [0xab; 32]);
/// assert_eq!(parse_pin(&"ab".repeat(32)).unwrap(), [0xab; 32]);
/// assert!(parse_pin("abcd").is_err());
/// ```
pub fn parse_pin(raw: &str) -> VaultResult<[u8; 32]> {
    let malformed = || {
        invalid(format!(
            "pin_sha256 must be a SHA-256 fingerprint, 64 hex characters (colons allowed), got \
             {raw:?}"
        ))
    };
    let hex: String = raw.trim().chars().filter(|c| *c != ':').collect();
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(malformed());
    }
    let mut pin = [0u8; 32];
    for (i, byte) in pin.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| malformed())?;
    }
    Ok(pin)
}

/// A rustls client config that trusts exactly one server certificate: the one whose SHA-256 is
/// `pin`. This is how SSH trusts a host key. No CA vouches for it, and the hostname and
/// validity dates are not checked, so a self-signed daemon certificate works. The handshake
/// signatures are still verified, so only the holder of that certificate's private key can
/// complete the connection.
pub fn pinned_client_config(pin: [u8; 32]) -> VaultResult<ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = PinVerifier {
        pin,
        provider: provider.clone(),
    };
    Ok(ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| VaultError::Generic(format!("tls: {e}")))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth())
}

/// Why [`PinVerifier`] refused a certificate, carried inside the rustls error so the message a
/// user sees says the pin didn't match rather than a generic verification failure.
struct PinMismatch;

impl std::fmt::Display for PinMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("server certificate doesn't match pin_sha256")
    }
}

// rustls prints `CertificateError::Other` with `{:?}` and gives its errors no `source()`, so
// `Debug` is what reaches the user: it says the same thing as `Display`.
impl std::fmt::Debug for PinMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for PinMismatch {}

/// Accepts the server's end-entity certificate only when its SHA-256 equals the pin.
#[derive(Debug)]
struct PinVerifier {
    pin: [u8; 32],
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let digest = Sha256::digest(end_entity.as_ref());
        match digest.as_slice() == self.pin {
            true => Ok(ServerCertVerified::assertion()),
            false => Err(rustls::Error::InvalidCertificate(CertificateError::Other(
                rustls::OtherError(Arc::new(PinMismatch)),
            ))),
        }
    }

    // The signature checks are what prove the server holds the pinned certificate's private
    // key. Delegated to the provider's real verification; never accepted unconditionally.
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
#[path = "tests/tls.rs"]
mod tests;
