use super::*;
use crate::{
    object::ObjectId,
    origin::{Origin, http::OriginHTTP},
};
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair};
use rustls::{
    ConfigBuilder, ServerConfig, SupportedProtocolVersion, WantsVerifier,
    crypto::CryptoProvider,
    pki_types::{CertificateDer, PrivateKeyDer},
    server::{ClientHello, ResolvesServerCert},
    sign::CertifiedKey,
};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Every version the client accepts. The server picks one, so a man in the middle could force
/// either: the pin checks must hold on both.
const VERSIONS: [&SupportedProtocolVersion; 2] = [&rustls::version::TLS13, &rustls::version::TLS12];

/// A server config builder that speaks only `version`.
fn server_builder(
    version: &'static SupportedProtocolVersion,
) -> ConfigBuilder<ServerConfig, WantsVerifier> {
    ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[version])
        .unwrap()
}

fn key_der(key: &KeyPair) -> PrivateKeyDer<'static> {
    PrivateKeyDer::Pkcs8(key.serialize_der().into())
}

/// A self-signed certificate for `names`, with its key.
fn self_signed(names: &[&str]) -> (CertificateDer<'static>, KeyPair) {
    let params =
        CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>()).unwrap();
    let key = KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap();
    (cert.der().clone(), key)
}

/// A CA (its PEM) and a leaf for 127.0.0.1 signed by it.
fn ca_and_leaf() -> (String, CertificateDer<'static>, KeyPair) {
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "nimbus test CA");
    let ca_key = KeyPair::generate().unwrap();
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();
    let issuer = Issuer::new(ca_params, ca_key);
    let leaf_params = CertificateParams::new(vec!["127.0.0.1".to_string()]).unwrap();
    let leaf_key = KeyPair::generate().unwrap();
    let leaf = leaf_params.signed_by(&leaf_key, &issuer).unwrap();
    (ca_cert.pem(), leaf.der().clone(), leaf_key)
}

/// An HTTPS server on 127.0.0.1 that answers every request with `[]`. Returns its base URL.
async fn serve(config: ServerConfig) -> String {
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(socket).await else {
                    return;
                };
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match tls.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = tls
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                          content-length: 2\r\nconnection: close\r\n\r\n[]",
                    )
                    .await;
                let _ = tls.shutdown().await;
            });
        }
    });
    format!("https://127.0.0.1:{}", address.port())
}

async fn serve_cert(
    cert: CertificateDer<'static>,
    key: &KeyPair,
    version: &'static SupportedProtocolVersion,
) -> String {
    let config = server_builder(version)
        .with_no_client_auth()
        .with_single_cert(vec![cert], key_der(key))
        .unwrap();
    serve(config).await
}

fn pin_of(cert: &CertificateDer<'_>) -> String {
    use sha2::{Digest, Sha256};
    // Upper case with colons, the way `openssl x509 -fingerprint -sha256` prints it.
    Sha256::digest(cert.as_ref())
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn pinned(pin: String) -> HttpTls {
    HttpTls {
        ca_file: None,
        pin_sha256: Some(pin),
    }
}

async fn can_list(origin: &OriginHTTP) -> bool {
    origin.list(&ObjectId::from("")).await.is_ok()
}

/// `error`'s message followed by each of its sources', one per line.
fn error_chain(error: &dyn std::error::Error) -> String {
    std::iter::successors(Some(error), |e| e.source())
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn a_matching_pin_connects_to_a_self_signed_server_no_ca_vouches_for() {
    for version in VERSIONS {
        // The name doesn't even match 127.0.0.1: a pin trusts the certificate itself.
        let (cert, key) = self_signed(&["some-other-host.example"]);
        let base = serve_cert(cert.clone(), &key, version).await;
        let origin = origin_at(base).with_tls(&pinned(pin_of(&cert))).unwrap();
        assert!(can_list(&origin).await, "{version:?}");
    }
}

#[tokio::test]
async fn a_wrong_pin_is_rejected() {
    let (cert, key) = self_signed(&["127.0.0.1"]);
    let (other, _) = self_signed(&["127.0.0.1"]);
    let base = serve_cert(cert, &key, &rustls::version::TLS13).await;
    let origin = origin_at(base).with_tls(&pinned(pin_of(&other))).unwrap();
    assert!(!can_list(&origin).await);
}

#[tokio::test]
async fn a_wrong_pin_explains_itself() {
    let (cert, key) = self_signed(&["127.0.0.1"]);
    let (other, _) = self_signed(&["127.0.0.1"]);
    let base = serve_cert(cert, &key, &rustls::version::TLS13).await;
    let origin = origin_at(base).with_tls(&pinned(pin_of(&other))).unwrap();

    let error = origin.list(&ObjectId::from("")).await.unwrap_err();
    let chain = error_chain(&error);
    assert!(chain.contains("doesn't match pin_sha256"), "{chain}");
}

/// Presents `cert` but signs the handshake with an unrelated key: someone who copied the
/// server's public certificate without having its private key.
#[derive(Debug)]
struct StolenCertificate(Arc<CertifiedKey>);
impl ResolvesServerCert for StolenCertificate {
    fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.0.clone())
    }
}

#[tokio::test]
async fn a_pinned_certificate_without_its_private_key_is_rejected() {
    for version in VERSIONS {
        // The pin matches, so only the handshake-signature check can catch this. A verifier
        // that waved signatures through would connect.
        let (cert, _real_key) = self_signed(&["127.0.0.1"]);
        let (_, impostor_key) = self_signed(&["127.0.0.1"]);
        let signing = provider()
            .key_provider
            .load_private_key(key_der(&impostor_key))
            .unwrap();
        let stolen = CertifiedKey::new(vec![cert.clone()], signing);
        let config = server_builder(version)
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(StolenCertificate(Arc::new(stolen))));
        let base = serve(config).await;
        let origin = origin_at(base).with_tls(&pinned(pin_of(&cert))).unwrap();
        assert!(!can_list(&origin).await, "{version:?}");
    }
}

#[tokio::test]
async fn ca_file_trusts_that_ca_and_only_that_ca() {
    let dir = tempfile::tempdir().unwrap();
    let (ca_pem, leaf, leaf_key) = ca_and_leaf();
    let (other_ca_pem, _, _) = ca_and_leaf();
    std::fs::write(dir.path().join("ca.pem"), ca_pem).unwrap();
    std::fs::write(dir.path().join("other.pem"), other_ca_pem).unwrap();
    let base = serve_cert(leaf, &leaf_key, &rustls::version::TLS13).await;
    let with_ca = |file: &str| HttpTls {
        ca_file: Some(dir.path().join(file)),
        pin_sha256: None,
    };

    let trusting = origin_at(base.clone())
        .with_tls(&with_ca("ca.pem"))
        .unwrap();
    assert!(can_list(&trusting).await);
    let wrong_ca = origin_at(base.clone())
        .with_tls(&with_ca("other.pem"))
        .unwrap();
    assert!(!can_list(&wrong_ca).await);
    // Without a tls table only the public roots are trusted, which don't know this CA.
    assert!(!can_list(&origin_at(base)).await);
    // The "only" half (public roots dropped once ca_file is set) can't be tested offline: no
    // public CA will sign a test certificate.
}

#[tokio::test]
async fn timeouts_set_after_or_before_tls_keep_the_tls_settings() {
    // `with_timeouts` used to rebuild the client from scratch, silently dropping TLS.
    let (cert, key) = self_signed(&["127.0.0.1"]);
    let base = serve_cert(cert.clone(), &key, &rustls::version::TLS13).await;
    let timeouts = (
        std::time::Duration::from_secs(5),
        std::time::Duration::from_secs(5),
    );

    let tls_then_timeouts = origin_at(base.clone())
        .with_tls(&pinned(pin_of(&cert)))
        .unwrap()
        .with_timeouts(timeouts.0, timeouts.1)
        .unwrap();
    assert!(can_list(&tls_then_timeouts).await);

    let timeouts_then_tls = origin_at(base)
        .with_timeouts(timeouts.0, timeouts.1)
        .unwrap()
        .with_tls(&pinned(pin_of(&cert)))
        .unwrap();
    assert!(can_list(&timeouts_then_tls).await);
}

fn tls_error(origin: OriginHTTP, tls: HttpTls) -> String {
    match origin.with_tls(&tls) {
        Ok(_) => panic!("{tls:?} should have been refused"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn invalid_tls_tables_are_refused_when_the_origin_is_built() {
    let dir = tempfile::tempdir().unwrap();
    let https = || origin_at("https://example.com".to_string());
    let good_pin = "ab".repeat(32);

    let both = tls_error(
        https(),
        HttpTls {
            ca_file: Some(dir.path().join("ca.pem")),
            pin_sha256: Some(good_pin.clone()),
        },
    );
    assert!(both.contains("only one of ca_file or pin_sha256"), "{both}");
    let neither = tls_error(https(), HttpTls::default());
    assert!(
        neither.contains("one of ca_file or pin_sha256"),
        "{neither}"
    );

    let plain_http = tls_error(
        origin_at("http://example.com".to_string()),
        pinned(good_pin),
    );
    assert!(
        plain_http.contains("need https URLs, but \"http://example.com/fetch/\""),
        "{plain_http}"
    );

    for bad in ["abcd", &"zz".repeat(32), &"ab".repeat(33), ""] {
        let message = tls_error(https(), pinned(bad.to_string()));
        assert!(message.contains("pin_sha256"), "{bad:?}: {message}");
    }

    let missing = dir.path().join("missing.pem");
    let message = tls_error(
        https(),
        HttpTls {
            ca_file: Some(missing.clone()),
            pin_sha256: None,
        },
    );
    assert!(
        message.contains(&missing.display().to_string()),
        "{message}"
    );

    let empty = dir.path().join("empty.pem");
    std::fs::write(&empty, "not a certificate\n").unwrap();
    let message = tls_error(
        https(),
        HttpTls {
            ca_file: Some(empty),
            pin_sha256: None,
        },
    );
    assert!(
        message.contains("no certificate") || message.contains("PEM"),
        "{message}"
    );
}

/// An origin whose `base_url` is empty and whose templates are the full URLs `scheme://host/…`,
/// except that `delete_url` uses `delete_scheme`.
fn origin_of_full_urls(scheme: &str, delete_scheme: &str) -> OriginHTTP {
    let template =
        |scheme: &str, operation: &str| format!("{scheme}://example.com/{operation}/{{id}}");
    OriginHTTP::new(
        String::new(),
        template(scheme, "fetch"),
        template(scheme, "list"),
        template(scheme, "get"),
        template(scheme, "put"),
        template(scheme, "send"),
        template(delete_scheme, "delete"),
    )
}

#[test]
fn tls_accepts_an_empty_base_url_when_the_templates_are_https() {
    let origin = origin_of_full_urls("https", "https");
    assert!(origin.with_tls(&pinned("ab".repeat(32))).is_ok());
}

#[test]
fn tls_refuses_when_any_template_is_plain_http() {
    let message = tls_error(
        origin_of_full_urls("https", "http"),
        pinned("ab".repeat(32)),
    );
    assert!(
        message.contains("\"http://example.com/delete/\" isn't one"),
        "{message}"
    );
}

/// `origin_of_full_urls("https", "https")` with a pin applied, ready for a rename URL.
fn https_origin_with_tls() -> OriginHTTP {
    origin_of_full_urls("https", "https")
        .with_tls(&pinned("ab".repeat(32)))
        .unwrap()
}

#[test]
fn a_plain_http_rename_url_added_after_tls_is_refused() {
    let message = match https_origin_with_tls()
        .with_rename_url(Some("http://example.com/rename/{id}".to_string()))
    {
        Ok(_) => panic!("a plain http rename_url should have been refused"),
        Err(e) => e.to_string(),
    };
    assert!(
        message.contains("\"http://example.com/rename/\" isn't one"),
        "{message}"
    );
}

#[test]
fn an_https_rename_url_added_after_tls_is_accepted() {
    let origin = https_origin_with_tls()
        .with_rename_url(Some("https://example.com/rename/{id}".to_string()));
    assert!(origin.is_ok());
}

#[test]
fn a_plain_http_rename_url_without_tls_is_allowed() {
    let origin = origin_of_full_urls("http", "http")
        .with_rename_url(Some("http://example.com/rename/{id}".to_string()));
    assert!(origin.is_ok());
}

#[test]
fn pins_accept_colons_and_either_case() {
    let lower = "ab".repeat(32);
    let upper_colons = vec!["AB"; 32].join(":");
    assert_eq!(
        parse_pin(&lower).unwrap(),
        parse_pin(&upper_colons).unwrap()
    );
}
