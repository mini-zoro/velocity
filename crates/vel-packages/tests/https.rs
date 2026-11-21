//! Real loopback TLS tests with a generated private CA. No public service or insecure TLS mode.
use sha2::{Digest, Sha256};
use std::{fs, io::Write, path::Path, time::Duration};
use vel_packages::{Index, PackageManager, https::HttpsClient};
use zip::{ZipWriter, write::SimpleFileOptions};
#[path = "support/tls.rs"]
mod tls;
use tls::Server;

fn server() -> Server {
    Server::start(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/https-server.py"))
}

fn package(path: &Path, version: &str) -> Vec<u8> {
    let mut zip = ZipWriter::new(fs::File::create(path).unwrap());
    zip.start_file("package-metadata.json", SimpleFileOptions::default())
        .unwrap();
    write!(zip, "{{\"version\":\"{version}\"}}").unwrap();
    zip.start_file("x.sublime-syntax", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"fixture syntax").unwrap();
    zip.finish().unwrap();
    fs::read(path).unwrap()
}

#[test]
fn verified_https_install_upgrade_digest_failures_and_redirect_policy() {
    let server = server();
    let root = tempfile::tempdir().unwrap();
    let archive = root.path().join("fixture.sublime-package");
    let original = package(&archive, "1.0.0");
    let mut manager = PackageManager::new(root.path().join("profile"));
    server.response(
        "package",
        "200 OK",
        "Content-Type: application/octet-stream\r\n",
        &original,
    );
    let client = server.client();
    let untrusted = HttpsClient {
        timeout: Duration::from_secs(5),
        ..HttpsClient::default()
    };
    assert!(
        manager
            .install_https("Example", &server.url("package"), &untrusted)
            .is_err()
    );
    let installed = manager
        .install_https("Example", &server.url("package"), &client)
        .unwrap();
    assert_eq!(fs::read(&installed.path).unwrap(), original);
    server.response("bad", "200 OK", "", b"not a ZIP");
    assert!(
        manager
            .install_https("Example", &server.url("bad"), &client)
            .is_err()
    );
    assert_eq!(fs::read(&installed.path).unwrap(), original);
    server.response("missing", "404 Not Found", "", b"missing");
    assert!(
        manager
            .install_https("Example", &server.url("missing"), &client)
            .is_err()
    );
    server.response(
        "redirect",
        "302 Found",
        &format!("Location: {}\r\n", server.url("package")),
        b"",
    );
    manager
        .install_https("Redirected", &server.url("redirect"), &client)
        .unwrap();
    server.response(
        "downgrade",
        "302 Found",
        "Location: http://127.0.0.1:1/insecure\r\n",
        b"",
    );
    assert!(
        manager
            .install_https("Example", &server.url("downgrade"), &client)
            .is_err()
    );
    assert!(
        manager
            .install_https("Example", "http://127.0.0.1:1/insecure", &client)
            .is_err()
    );
    let temp = tempfile::NamedTempFile::new().unwrap();
    assert!(
        client
            .download(&server.url("package"), temp.path(), 16)
            .is_err()
    );
    fs::write(
        server.root.path().join("unbounded"),
        [
            b"HTTP/1.0 200 OK\r\nConnection: close\r\n\r\n".as_slice(),
            &[b'x'; 1024],
        ]
        .concat(),
    )
    .unwrap();
    assert!(
        client
            .download(&server.url("unbounded"), temp.path(), 16)
            .is_err()
    );
    assert!(fs::metadata(temp.path()).unwrap().len() <= 17);

    let upgraded = package(&archive, "1.10.0");
    server.response("upgrade", "200 OK", "", &upgraded);
    let index_json = serde_json::json!({"schema_version":1, "packages":[{
        "name":"Example", "version":"1.10.0", "url":server.url("upgrade"),
        "sha256":Sha256::digest(&upgraded).iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
    }]});
    let index_bytes = serde_json::to_vec(&index_json).unwrap();
    server.response(
        "index",
        "200 OK",
        "Content-Type: application/json\r\n",
        &index_bytes,
    );
    assert!(manager.check_upgrades(&client).unwrap().is_empty());
    manager.set_index_url(Some(server.url("index"))).unwrap();
    let upgrades = manager.check_upgrades(&client).unwrap();
    assert_eq!(upgrades.len(), 1);
    let mut bad = upgrades[0].available.clone();
    bad.sha256 = "0".repeat(64);
    assert!(manager.install_release(&bad, &client).is_err());
    assert_eq!(fs::read(&installed.path).unwrap(), original);
    let mut mismatched = upgrades[0].available.clone();
    mismatched.version = "1.11.0".parse().unwrap();
    assert!(manager.install_release(&mismatched, &client).is_err());
    let replacement = manager
        .install_release(&upgrades[0].available, &client)
        .unwrap();
    assert_eq!(fs::read(replacement.backup.unwrap()).unwrap(), original);
    assert_eq!(fs::read(&replacement.path).unwrap(), upgraded);
    assert!(manager.check_upgrades(&client).unwrap().is_empty());
    assert!(Index::parse(&index_bytes).is_ok());
    manager.set_index_url(None).unwrap();
    assert!(manager.check_upgrades(&client).unwrap().is_empty());
}

#[test]
fn r35_anonymous_https_bounds_bytes_redirects_and_credentials() {
    let server = server();
    let client = HttpsClient {
        timeout: Duration::from_secs(30),
        ..server.client()
    };
    server.response("pixel", "200 OK", "Set-Cookie: fixture=x\r\n", b"PNG");
    server.response(
        "redirect",
        "302 Found",
        "Location: /pixel\r\nSet-Cookie: fixture=y\r\n",
        b"",
    );
    assert_eq!(
        client.fetch_anonymous(&server.url("redirect"), 16).unwrap(),
        b"PNG"
    );
    assert!(client.fetch_anonymous(&server.url("pixel"), 2).is_err());
    assert!(client.fetch_anonymous(&server.url("pixel"), 16).is_ok());
    let requests = || fs::read_to_string(server.root.path().join("requests.log")).unwrap();
    assert!(!requests().contains("Cookie"));
    assert!(!requests().contains("Referer"));

    server.response("credential-target", "200 OK", "", b"secret");
    let credential_url =
        server
            .url("credential-target")
            .replacen("https://", "https://alice:secret@", 1);
    server.response(
        "credentials",
        "302 Found",
        &format!("Location: {credential_url}\r\n"),
        b"",
    );
    assert!(client.fetch_anonymous(&credential_url, 16).is_err());
    assert!(
        client
            .fetch_anonymous(&server.url("credentials"), 16)
            .is_err()
    );
    assert!(!requests().contains("credential-target"));
    assert!(!requests().contains("Authorization"));

    for i in 0..4 {
        server.response(
            &format!("hop{i}"),
            "302 Found",
            &format!("Location: /hop{}\r\n", i + 1),
            b"",
        );
    }
    server.response("hop4", "200 OK", "", b"too far");
    assert!(client.fetch_anonymous(&server.url("hop0"), 16).is_err());
    assert!(!requests().contains("hop4"));

    server.response(
        "downgrade",
        "302 Found",
        "Location: http://127.0.0.1:1/no\r\n",
        b"",
    );
    assert!(
        client
            .fetch_anonymous(&server.url("downgrade"), 16)
            .is_err()
    );
    assert!(client.fetch_anonymous("http://127.0.0.1:1/no", 16).is_err());
    assert!(
        HttpsClient::default()
            .fetch_anonymous(&server.url("pixel"), 16)
            .is_err()
    );

    fs::write(
        server.root.path().join("chunked"),
        [
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n40\r\n"
                .as_slice(),
            &[b'x'; 64],
            b"\r\n0\r\n\r\n",
        ]
        .concat(),
    )
    .unwrap();
    assert!(client.fetch_anonymous(&server.url("chunked"), 16).is_err());

    let deadline_client = HttpsClient {
        timeout: Duration::from_secs(8),
        ..client
    };
    let error = deadline_client
        .fetch_anonymous(&server.url("stall"), 16)
        .unwrap_err();
    assert!(error.to_string().contains("exit status: 28"), "{error}");
}
