use super::*;
use crate::{api, auth::Auth, server::SHUTDOWN_GRACE, state::AppState};
use nimbus_vault::{
    object::ObjectId,
    origin::{
        Origin,
        fs::OriginFileSystem,
        http::{HttpTls, OriginHTTP},
    },
    vault::Vault,
};
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair};
use std::{collections::HashMap, path::Path, sync::Arc};

/// A test CA: its certificate in PEM, and what it signs leaf certificates with.
struct TestCa {
    pem: String,
    issuer: Issuer<'static, KeyPair>,
}

fn new_ca(name: &str) -> TestCa {
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.distinguished_name.push(DnType::CommonName, name);
    let key = KeyPair::generate().unwrap();
    let pem = params.self_signed(&key).unwrap().pem();
    TestCa {
        pem,
        issuer: Issuer::new(params, key),
    }
}

/// Writes a leaf certificate for 127.0.0.1 signed by `ca`, and its key, as the files `tls`
/// names.
fn write_leaf(ca: &TestCa, tls: &TlsConfig) {
    let params = CertificateParams::new(vec!["127.0.0.1".to_string()]).unwrap();
    let key = KeyPair::generate().unwrap();
    let cert = params.signed_by(&key, &ca.issuer).unwrap();
    std::fs::write(&tls.cert_file, cert.pem()).unwrap();
    std::fs::write(&tls.key_file, key.serialize_pem()).unwrap();
}

fn tls_files(dir: &Path) -> TlsConfig {
    TlsConfig {
        cert_file: dir.join("cert.pem"),
        key_file: dir.join("key.pem"),
    }
}

/// Serves one fs vault named `v` (holding `notes.txt`) over HTTPS with `tls`. Returns the base
/// URL and the live TLS config, for reload tests.
async fn serve_vault(dir: &Path, tls: &TlsConfig) -> (String, RustlsConfig) {
    let root = dir.join("vault");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("notes.txt"), b"hello").unwrap();
    let vault = Vault::from_parts(
        "v".to_string(),
        Arc::new(OriginFileSystem::new(root)),
        ObjectId::from(""),
    )
    .unwrap();
    let state = AppState::from_parts(HashMap::from([("v".to_string(), vault)]), false, Auth::None);

    let https = Https::load(tls).await.unwrap();
    let config = https.rustls_config();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(https.serve(
        listener,
        api::router(state),
        std::future::pending(),
        SHUTDOWN_GRACE,
    ));
    (format!("https://127.0.0.1:{port}"), config)
}

fn client(base: &str) -> OriginHTTP {
    OriginHTTP::new(
        format!("{base}/v/v"),
        "/fetch/{id}".to_string(),
        "/list/{id}".to_string(),
        "/get/{id}".to_string(),
        "/put/{id}".to_string(),
        "/send/{id}".to_string(),
        "/delete/{id}".to_string(),
    )
}

/// A client that trusts only the CA in `ca_pem`, written next to the test's other files.
fn client_trusting(base: &str, dir: &Path, ca_pem: &str) -> OriginHTTP {
    let ca_file = dir.join(format!("ca-{}.pem", ca_pem.len()));
    std::fs::write(&ca_file, ca_pem).unwrap();
    client(base)
        .with_tls(&HttpTls {
            ca_file: Some(ca_file),
            pin_sha256: None,
        })
        .unwrap()
}

async fn lists_notes(origin: &OriginHTTP) -> bool {
    match origin.list(&ObjectId::from("")).await {
        Ok(objects) => objects.iter().any(|o| o.get_name() == "notes.txt"),
        Err(_) => false,
    }
}

#[tokio::test]
async fn https_serves_a_vault_to_a_client_that_trusts_the_ca() {
    let dir = tempfile::tempdir().unwrap();
    let ca = new_ca("nimbus test CA");
    let tls = tls_files(dir.path());
    write_leaf(&ca, &tls);
    let (base, _) = serve_vault(dir.path(), &tls).await;

    assert!(lists_notes(&client_trusting(&base, dir.path(), &ca.pem)).await);
}

#[tokio::test]
async fn a_client_without_the_ca_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let ca = new_ca("nimbus test CA");
    let tls = tls_files(dir.path());
    write_leaf(&ca, &tls);
    let (base, _) = serve_vault(dir.path(), &tls).await;

    // Only the public roots: an unknown issuer.
    let error = client(&base).list(&ObjectId::from("")).await.unwrap_err();
    assert!(
        matches!(error, nimbus_vault::error::VaultError::HTTP(_)),
        "{error}"
    );
}

#[tokio::test]
async fn plain_http_to_the_tls_port_fails() {
    let dir = tempfile::tempdir().unwrap();
    let ca = new_ca("nimbus test CA");
    let tls = tls_files(dir.path());
    write_leaf(&ca, &tls);
    let (base, _) = serve_vault(dir.path(), &tls).await;

    let plain = base.replacen("https://", "http://", 1);
    assert!(client(&plain).list(&ObjectId::from("")).await.is_err());
}

#[tokio::test]
async fn reload_swaps_the_certificate_for_new_connections() {
    let dir = tempfile::tempdir().unwrap();
    let (old_ca, new_ca) = (new_ca("old CA"), new_ca("new CA"));
    let tls = tls_files(dir.path());
    write_leaf(&old_ca, &tls);
    let (base, config) = serve_vault(dir.path(), &tls).await;
    assert!(lists_notes(&client_trusting(&base, dir.path(), &old_ca.pem)).await);

    // A renewed certificate from a different CA lands in the same files, then a reload.
    write_leaf(&new_ca, &tls);
    reload(&config, &tls).await.unwrap();

    assert!(lists_notes(&client_trusting(&base, dir.path(), &new_ca.pem)).await);
    assert!(!lists_notes(&client_trusting(&base, dir.path(), &old_ca.pem)).await);
}

#[tokio::test]
async fn a_failed_reload_keeps_serving_the_old_certificate() {
    let dir = tempfile::tempdir().unwrap();
    let ca = new_ca("nimbus test CA");
    let tls = tls_files(dir.path());
    write_leaf(&ca, &tls);
    let (base, config) = serve_vault(dir.path(), &tls).await;

    std::fs::write(&tls.key_file, "half-written key").unwrap();
    let error = reload(&config, &tls).await.unwrap_err();
    assert!(
        format!("{error:#}").contains(&tls.key_file.display().to_string()),
        "{error:#}"
    );
    assert!(lists_notes(&client_trusting(&base, dir.path(), &ca.pem)).await);
}

/// The error [`Https::load`] gives for `tls`, with its context chain.
async fn load_error(tls: &TlsConfig) -> String {
    match Https::load(tls).await {
        Ok(_) => panic!("{tls:?} should have failed to load"),
        Err(e) => format!("{e:#}"),
    }
}

#[tokio::test]
async fn load_errors_name_the_files_involved() {
    let dir = tempfile::tempdir().unwrap();
    let tls = tls_files(dir.path());

    // Missing certificate.
    let error = load_error(&tls).await;
    assert!(
        error.contains(&tls.cert_file.display().to_string()),
        "{error}"
    );

    // A key that doesn't belong to the certificate.
    let ca = new_ca("nimbus test CA");
    write_leaf(&ca, &tls);
    let stranger = KeyPair::generate().unwrap();
    std::fs::write(&tls.key_file, stranger.serialize_pem()).unwrap();
    let error = load_error(&tls).await;
    assert!(
        error.contains(&tls.cert_file.display().to_string()),
        "{error}"
    );
    assert!(
        error.contains(&tls.key_file.display().to_string()),
        "{error}"
    );

    // Something that isn't PEM at all.
    std::fs::write(&tls.cert_file, "not a certificate").unwrap();
    assert!(Https::load(&tls).await.is_err());
}
