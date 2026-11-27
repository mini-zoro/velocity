//! A single anonymous GET. The caller, not curl, decides whether to follow a redirect.
use super::{HttpsClient, validate_url};
use crate::{Result, invalid};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
};

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub etag: Option<String>,
    pub location: Option<String>,
}

impl HttpsClient {
    pub fn get(
        &self,
        url: &str,
        agent: &str,
        etag: Option<&str>,
        destination: &mut impl Write,
        max_bytes: u64,
    ) -> Result<Response> {
        validate_url(url)?;
        self.get_once(url, "=https", agent, etag, destination, max_bytes)
    }

    #[cfg(feature = "test-loopback-http")]
    #[doc(hidden)]
    pub fn get_loopback(
        &self,
        url: &str,
        agent: &str,
        etag: Option<&str>,
        destination: &mut impl Write,
        max_bytes: u64,
    ) -> Result<Response> {
        let (authority, _) = url
            .strip_prefix("http://127.0.0.1:")
            .and_then(|tail| tail.split_once('/'))
            .ok_or_else(|| invalid("expected test loopback URL"))?;
        authority
            .parse::<u16>()
            .map_err(|_| invalid("invalid test port"))?;
        self.get_once(url, "=http", agent, etag, destination, max_bytes)
    }

    fn get_once(
        &self,
        url: &str,
        protocol: &str,
        agent: &str,
        etag: Option<&str>,
        destination: &mut impl Write,
        max_bytes: u64,
    ) -> Result<Response> {
        if self.timeout.is_zero()
            || max_bytes == 0
            || max_bytes == u64::MAX
            || !printable(agent, 128)
            || etag.is_some_and(|v| !printable(v, 256))
        {
            return Err(invalid("invalid GET options"));
        }
        #[cfg(target_os = "macos")]
        let executable = "/usr/bin/curl";
        #[cfg(windows)]
        let executable = std::env::var_os("SystemRoot")
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_absolute())
            .ok_or_else(|| invalid("cannot locate system curl.exe"))?
            .join("System32/curl.exe");
        #[cfg(not(any(target_os = "macos", windows)))]
        let executable = "curl";
        let mut command = Command::new(executable);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        command
            .args([
                "--disable",
                "--globoff",
                "--silent",
                "--include",
                "--noproxy",
                "*",
                "--disallow-username-in-url",
                "--proto",
                protocol,
                "--user-agent",
                agent,
                "--connect-timeout",
                "5",
                "--max-time",
                &self.timeout.as_secs_f64().to_string(),
                "--max-filesize",
                &max_bytes.to_string(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for key in [
            "CURL_CA_BUNDLE",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
            "SSLKEYLOGFILE",
        ] {
            command.env_remove(key);
        }
        if let Some(ca) = &self.ca_certificate {
            command.arg("--cacert").arg(ca);
        }
        if let Some(etag) = etag {
            command
                .arg("--header")
                .arg(format!("If-None-Match: {etag}"));
        }
        let mut child = command.arg("--url").arg(url).spawn()?;
        let mut stream = BufReader::new(child.stdout.take().expect("piped curl stdout"));
        let read_result = (|| {
            let mut budget = 32 * 1024;
            let response = loop {
                let response = read_headers(&mut stream, &mut budget)?;
                if !(100..200).contains(&response.status) {
                    break response;
                }
            };
            let body = stream.take(max_bytes + 1);
            let size = if response.status == 200 {
                io::copy(&mut { body }, destination)?
            } else {
                io::copy(&mut { body }, &mut io::sink())?
            };
            if size > max_bytes {
                return Err(invalid("GET response exceeds size limit"));
            }
            Ok(response)
        })();
        if read_result.is_err() {
            let _ = child.kill();
        }
        let status = child.wait()?;
        let response = read_result?;
        if !status.success() {
            return Err(invalid(format!("GET failed (curl status {status})")));
        }
        Ok(response)
    }
}

fn printable(header: &str, limit: usize) -> bool {
    !header.is_empty()
        && header.len() <= limit
        && header.bytes().all(|b| (b' '..=b'~').contains(&b))
}

fn read_headers(stream: &mut impl BufRead, budget: &mut u64) -> Result<Response> {
    let invalid_header = || invalid("invalid or oversized GET response headers");
    let mut read_line = |line: &mut Vec<u8>| -> Result<()> {
        line.clear();
        let size = stream.take(*budget).read_until(b'\n', line)? as u64;
        *budget -= size;
        if !line.ends_with(b"\r\n") {
            return Err(invalid_header());
        }
        line.truncate(line.len() - 2);
        Ok(())
    };
    let mut line = Vec::new();
    read_line(&mut line)?;
    let status = std::str::from_utf8(&line)
        .ok()
        .and_then(|s| s.strip_prefix("HTTP/"))
        .and_then(|s| s.split_ascii_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(invalid_header)?;
    let mut response = Response {
        status,
        etag: None,
        location: None,
    };
    loop {
        read_line(&mut line)?;
        if line.is_empty() {
            return Ok(response);
        }
        let Some(colon) = line.iter().position(|&b| b == b':') else {
            return Err(invalid_header());
        };
        let (name, value) = (&line[..colon], &line[colon + 1..]);
        let (field, limit) = if name.eq_ignore_ascii_case(b"etag") {
            (&mut response.etag, 256)
        } else if name.eq_ignore_ascii_case(b"location") {
            (&mut response.location, 8192)
        } else {
            continue;
        };
        let value = std::str::from_utf8(value)
            .map_err(|_| invalid_header())?
            .trim_matches([' ', '\t']);
        if !printable(value, limit) || field.replace(value.into()).is_some() {
            return Err(invalid_header());
        }
    }
}
