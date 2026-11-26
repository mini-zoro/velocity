use super::*;
use ed25519_dalek::{Signer, SigningKey};
use std::{
    io::{BufRead, BufReader, Cursor, Write},
    net::TcpListener,
    path::PathBuf,
};

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

#[test]
fn semver_and_release_channels() {
    let mut release: Release = serde_json::from_str(
        r#"{
        "tag_name":"v0.10.0","draft":false,"prerelease":false,"assets":[],"body":"ignored"
    }"#,
    )
    .unwrap();
    assert_eq!(
        candidate(&release, &v("0.9.19")).unwrap(),
        Some(v("0.10.0"))
    );
    assert_eq!(candidate(&release, &v("0.10.0+local")).unwrap(), None);
    assert_eq!(candidate(&release, &v("1.0.0")).unwrap(), None);
    release.draft = true;
    assert_eq!(candidate(&release, &v("0.9.19")).unwrap(), None);
    release.draft = false;
    release.prerelease = true;
    assert_eq!(candidate(&release, &v("0.9.19")).unwrap(), None);
    release.prerelease = false;
    release.tag_name = "v99.0.0-rc.1".into();
    assert_eq!(candidate(&release, &v("0.9.19")).unwrap(), None);
    release.tag_name = "v99.0".into();
    assert_eq!(
        candidate(&release, &v("0.9.19")).unwrap_err().reason,
        Reason::Release
    );
}

#[test]
fn asset_names_and_origins_are_exact() {
    let mut release = Release {
        tag_name: "v0.2.0".into(),
        draft: false,
        prerelease: false,
        assets: vec![],
    };
    for name in [
        "Velocity-0.2.0-windows-x64.zip",
        "Velocity-0.2.0-universal.app.zip",
        "Velocity-0.2.0-windows-x64-setup.exe",
        "Velocity-0.2.0-universal.dmg",
    ] {
        release.assets.push(Asset {
            name: name.into(),
            browser_download_url: format!("{RELEASES}/download/v0.2.0/{name}"),
        });
    }
    for (platform, suffix) in [
        (Platform::Mac, "universal.app.zip"),
        (Platform::Windows, "windows-x64.zip"),
    ] {
        let client = Client::new("0.1.3", platform).unwrap();
        let name = platform.asset(&v("0.2.0"));
        assert!(client.asset_url(&release, &name).unwrap().ends_with(suffix));
        let mut bad = release.clone();
        bad.assets
            .push(bad.assets.iter().find(|a| a.name == name).unwrap().clone());
        assert!(client.asset_url(&bad, &name).is_err());
        let mut bad = release.clone();
        bad.assets
            .iter_mut()
            .find(|a| a.name == name)
            .unwrap()
            .browser_download_url = "https://unrelated.example/asset".into();
        assert!(client.asset_url(&bad, &name).is_err());
    }
    assert!(allowed_redirect(
        "https://release-assets.githubusercontent.com/download?token=abc"
    ));
    for url in [
        "https://github.com/another/repo",
        "http://release-assets.githubusercontent.com/download",
        "https://release-assets.githubusercontent.com.evil.example/download",
        "https://objects.githubusercontent.com@evil.example/download",
    ] {
        assert!(!allowed_redirect(url));
    }
}

fn signed(asset: &[u8], name: &str, key: &SigningKey) -> (Vec<u8>, Vec<u8>) {
    let hex: String = verify::digest(asset)
        .unwrap()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let sums = format!("{hex}  {name}\n").into_bytes();
    let sig = key.sign(&sums).to_bytes().to_vec();
    (sums, sig)
}

#[test]
fn fail_closed_verification() {
    let key = SigningKey::from_bytes(&[7; 32]);
    let public = key.verifying_key().to_bytes();
    let asset = "fixture.zip";
    let bytes = b"the authentic archive";
    let (mut sums, sig) = signed(bytes, asset, &key);
    let expected = verify::signed_digest(&sums, &sig, &[public], asset).unwrap();
    verify::check_digest(&bytes[..], &expected).unwrap();
    assert_eq!(
        verify::check_digest(&b"tampered"[..], &expected)
            .unwrap_err()
            .reason,
        Reason::Checksum
    );
    assert_eq!(
        verify::signed_digest(&sums, &[], &[public], asset)
            .unwrap_err()
            .reason,
        Reason::MissingSignature
    );
    let wrong_key = SigningKey::from_bytes(&[8; 32]).verifying_key().to_bytes();
    assert_eq!(
        verify::signed_digest(&sums, &sig, &[wrong_key], asset)
            .unwrap_err()
            .reason,
        Reason::Signature
    );
    assert!(verify::signed_digest(&sums, &sig, &[wrong_key, public], asset).is_ok());
    assert_eq!(
        verify::signed_digest(&sums, &sig, &[public], "wrong.zip")
            .unwrap_err()
            .reason,
        Reason::Checksum
    );
    sums[0] = if sums[0] == b'a' { b'b' } else { b'a' };
    assert_eq!(
        verify::signed_digest(&sums, &sig, &[public], asset)
            .unwrap_err()
            .reason,
        Reason::Signature
    );
    sums.extend(sums.clone());
    let sig = key.sign(&sums).to_bytes();
    assert_eq!(
        verify::signed_digest(&sums, &sig, &[public], asset)
            .unwrap_err()
            .reason,
        Reason::Checksum
    );
}

fn portable_zip(version: &str) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (path, bytes) in super::super::install::tests::portable_files(version) {
        writer
            .start_file(
                format!("Velocity/{path}"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.write_all(&bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn loopback_latest_etag_schedule_skip_and_stage() {
    let server = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let base = format!("http://{}", server.local_addr().unwrap());
    let name = "Velocity-0.2.0-windows-x64.zip";
    let key = SigningKey::from_bytes(&[7; 32]);
    let zip = portable_zip("0.2.0");
    let (sums, sig) = signed(&zip, name, &key);
    let asset = |name: &str| format!("/download/v0.2.0/{name}");
    let assets = [name, "SHA256SUMS", "SHA256SUMS.sig"].map(|name| {
        serde_json::json!({"name":name,"browser_download_url":format!("{base}{}", asset(name))})
    });
    let release =
        serde_json::json!({"tag_name":"v0.2.0","draft":false,"prerelease":false,"assets":assets});
    let requests = [
        (
            "/latest".to_owned(),
            false,
            200,
            release.to_string().into_bytes(),
        ),
        (asset("SHA256SUMS"), false, 200, sums.clone()),
        (asset("SHA256SUMS.sig"), false, 200, sig.clone()),
        (asset(name), false, 200, zip),
        ("/latest".into(), true, 304, Vec::new()),
        (asset("SHA256SUMS"), false, 200, sums),
        (asset("SHA256SUMS.sig"), false, 200, sig),
        ("/latest".into(), true, 304, Vec::new()),
    ];
    let thread = std::thread::spawn(move || {
        server.set_nonblocking(true).unwrap();
        for (path, etag, status, bytes) in requests {
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            let mut client = loop {
                match server.accept() {
                    Ok((client, _)) => break client,
                    Err(e)
                        if e.kind() == io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(2))
                    }
                    Err(e) => panic!("awaiting {path}: {e}"),
                }
            };
            client
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let headers = BufReader::new(&client)
                .lines()
                .map(|l| l.unwrap())
                .take_while(|line| !line.is_empty())
                .collect::<Vec<_>>();
            assert_eq!(headers.first().unwrap(), &format!("GET {path} HTTP/1.1"));
            let headers = headers.join("\n").to_ascii_lowercase();
            assert!(headers.contains("user-agent: velocity/0.1.3"));
            assert_eq!(headers.contains("if-none-match: w/\"v2\""), etag);
            assert!(
                !headers.contains("authorization:")
                    && !headers.contains("cookie:")
                    && !headers.contains("referer:")
            );
            let validator = if path == "/latest" {
                "ETag: W/\"v2\"\r\n"
            } else {
                ""
            };
            write!(client, "HTTP/1.1 {status} Response\r\nContent-Length: {}\r\n{validator}Connection: close\r\n\r\n", bytes.len()).unwrap();
            client.write_all(&bytes).unwrap();
        }
    });
    let data = tempfile::tempdir().unwrap();
    let installed = tempfile::tempdir().unwrap();
    let mut client = Client::new("0.1.3", Platform::Windows).unwrap();
    client.fixture = Some((base, key.verifying_key().to_bytes()));
    let now = UNIX_EPOCH + Duration::from_secs(100_000);
    let mut state = State::load(data.path());
    let Outcome::Available(offer) = client.check(data.path(), &mut state, now, false).unwrap()
    else {
        panic!("expected verified release")
    };
    let files = super::super::install::tests::portable_files("0.1.3");
    let portable = installed.path().join("Velocity");
    for (path, bytes) in files {
        let path: PathBuf = portable.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    let install =
        crate::Install::detect(&portable.join("velocity.exe"), Platform::Windows, "0.1.3").unwrap();
    let _staged = install.stage(&client, data.path(), offer).unwrap();
    let mut state = State::load(data.path());
    assert_eq!(state.due_in(now), CHECK_INTERVAL);
    let before = now + CHECK_INTERVAL - Duration::from_secs(1);
    assert!(matches!(
        client
            .check(data.path(), &mut state, before, false)
            .unwrap(),
        Outcome::NotDue
    ));
    assert!(matches!(
        client
            .check(data.path(), &mut state, now + CHECK_INTERVAL, false)
            .unwrap(),
        Outcome::Available(_)
    ));
    State::skip(data.path(), v("0.2.0")).unwrap();
    let mut state = State::load(data.path());
    assert!(
        matches!(client.check(data.path(), &mut state, now + CHECK_INTERVAL, true).unwrap(), Outcome::Skipped(version) if version == v("0.2.0"))
    );
    thread.join().unwrap();
}
