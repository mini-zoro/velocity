//! Bounded, certificate-verified HTTPS using macOS's system `curl` (no shell and no plugins).
use crate::{Result, invalid};
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

/// Explicit transport settings. Default TLS verification is always enabled.
#[derive(Clone, Debug)]
pub struct HttpsClient {
    /// Optional private CA PEM, for an explicitly configured enterprise or local test server.
    pub ca_certificate: Option<PathBuf>,
    /// Total request deadline, including redirects and downloads.
    pub timeout: Duration,
}
impl Default for HttpsClient {
    fn default() -> Self {
        Self {
            ca_certificate: None,
            timeout: Duration::from_secs(60),
        }
    }
}
impl HttpsClient {
    /// Writes to an existing temporary file; callers validate and commit it afterwards.
    /// Redirects are restricted to HTTPS, `.curlrc` is ignored, and response bodies are bounded.
    pub fn download(&self, url: &str, destination: &Path, max_bytes: u64) -> Result<()> {
        validate_url(url)?;
        if self.timeout.is_zero() || max_bytes == 0 || max_bytes == u64::MAX {
            return Err(invalid("invalid HTTPS limits"));
        }
        #[cfg(target_os = "macos")]
        let executable = "/usr/bin/curl";
        #[cfg(not(target_os = "macos"))]
        let executable = "curl";
        let mut command = Command::new(executable);
        command
            .args([
                "--disable",
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                "--max-redirs",
                "5",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--noproxy",
                "localhost,127.0.0.1,::1",
                "--connect-timeout",
                "10",
                "--max-time",
                &self.timeout.as_secs_f64().to_string(),
                "--max-filesize",
                &max_bytes.to_string(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(ca) = &self.ca_certificate {
            command.arg("--cacert").arg(ca);
        }
        // Independently bound the stream: older system curls only enforce --max-filesize
        // when Content-Length is present. Limit disk writes even for chunked responses.
        let mut file = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(destination)?;
        let mut child = command.arg("--url").arg(url).spawn()?;
        let mut stream = child
            .stdout
            .take()
            .expect("piped curl stdout")
            .take(max_bytes + 1);
        let copied = io::copy(&mut stream, &mut file);
        if !copied.as_ref().is_ok_and(|size| *size <= max_bytes) {
            let _ = child.kill();
            let _ = child.wait();
            copied?;
            return Err(invalid("HTTPS response exceeds size limit"));
        }
        let status = child.wait()?;
        if !status.success() {
            return Err(invalid(format!(
                "HTTPS download failed (curl status {status})"
            )));
        }
        Ok(())
    }
}
pub(crate) fn validate_url(url: &str) -> Result<()> {
    let authority = url
        .strip_prefix("https://")
        .unwrap_or("")
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("");
    if authority.is_empty()
        || authority.contains('@')
        || url.chars().any(char::is_whitespace)
        || url.contains('\0')
    {
        return Err(invalid("a credential-free HTTPS URL is required"));
    }
    Ok(())
}
