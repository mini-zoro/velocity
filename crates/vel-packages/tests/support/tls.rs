//! Shared loopback TLS fixture. Never uses the user's trust store for the server certificate.
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use vel_packages::https::HttpsClient;

#[derive(Debug)]
pub struct Server {
    pub root: tempfile::TempDir,
    child: Child,
    base: String,
    ca: PathBuf,
}
impl Server {
    pub fn start(script: &Path) -> Self {
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
            .arg(script)
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
        let deadline = Instant::now() + Duration::from_secs(30);
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
    pub fn response(&self, name: &str, status: &str, headers: &str, body: &[u8]) {
        let mut file = fs::File::create(self.root.path().join(name)).unwrap();
        write!(
            file,
            "HTTP/1.0 {status}\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        file.write_all(body).unwrap();
    }
    pub fn url(&self, name: &str) -> String {
        format!("{}/{name}", self.base)
    }
    pub fn client(&self) -> HttpsClient {
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
