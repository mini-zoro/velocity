//! Real loopback TLS tests with a generated private CA. No public service or insecure TLS mode.
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use vel_packages::{Index, PackageManager, https::HttpsClient};
use zip::{ZipWriter, write::SimpleFileOptions};

#[derive(Debug)]
struct Server {
    root: tempfile::TempDir,
    child: Child,
    base: String,
    ca: PathBuf,
}
impl Server {
    fn start() -> Self {
        let root = tempfile::tempdir().unwrap();
        let ca = root.path().join("certificate.pem");
        let key = root.path().join("key.pem");
        let config = root.path().join("openssl.cnf");
        fs::write(&config, "[req]\ndistinguished_name=dn\nx509_extensions=extensions\nprompt=no\n[dn]\nCN=localhost\n[extensions]\nsubjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,digitalSignature,keyEncipherment,keyCertSign\nextendedKeyUsage=serverAuth\n").unwrap();
        let status = Command::new(openssl())
            .args([
                "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-sha256", "-days", "1", "-config",
            ])
            .arg(&config)
            .arg("-keyout")
            .arg(&key)
            .arg("-out")
            .arg(&ca)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        let server_log = root.path().join("server.log");
        let ready = root.path().join("ready");
        let python = if cfg!(target_os = "macos") {
            "/usr/bin/python3"
        } else {
            "python3"
        };
        let mut child = Command::new(python)
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/https-server.py"))
            .arg(root.path())
            .arg(&ca)
            .arg(&key)
            .arg(&ready)
            .current_dir(root.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(&server_log).unwrap())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        // The fixture creates the ready file before it writes the port, so wait until
        // the content parses instead of only until the file exists.
        let port: u16 = loop {
            if let Some(port) = fs::read_to_string(&ready)
                .ok()
                .and_then(|text| text.trim().parse().ok())
            {
                break port;
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "OpenSSL server exited: {}",
                fs::read_to_string(&server_log).unwrap()
            );
            assert!(Instant::now() < deadline, "TLS fixture did not bind");
            std::thread::sleep(Duration::from_millis(10));
        };
        Self {
            root,
            child,
            base: format!("https://localhost:{port}"),
            ca,
        }
    }
    fn response(&self, name: &str, status: &str, headers: &str, body: &[u8]) {
        let mut file = fs::File::create(self.root.path().join(name)).unwrap();
        write!(
            file,
            "HTTP/1.0 {status}\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        file.write_all(body).unwrap();
    }
    fn url(&self, name: &str) -> String {
        format!("{}/{name}", self.base)
    }
    fn client(&self) -> HttpsClient {
        HttpsClient {
            ca_certificate: Some(self.ca.clone()),
            timeout: Duration::from_secs(5),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn openssl() -> &'static str {
    if cfg!(target_os = "macos") {
        "/usr/bin/openssl"
    } else {
        "openssl"
    }
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
    let server = Server::start();
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
