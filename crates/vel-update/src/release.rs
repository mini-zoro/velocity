use crate::{Error, Platform, RELEASES, Reason, Result, verify};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use vel_packages::https::{HttpsClient, Response};

pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const REPO: &str = "mini-zoro/velocity";
const API: &str = "https://api.github.com/repos/mini-zoro/velocity/releases/latest";
const METADATA_LIMIT: u64 = 1024 * 1024;
pub(crate) const ARCHIVE_LIMIT: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

#[derive(Clone, Debug)]
pub struct Offer {
    pub version: Version,
    pub(crate) platform: Platform,
    pub(crate) asset_url: String,
    pub(crate) digest: [u8; 32],
    pub(crate) proof: (Vec<u8>, Vec<u8>),
}

impl Offer {
    pub fn notes(&self) -> String {
        format!(
            "{RELEASES}/tag/v{}",
            self.version.to_string().replace('+', "%2B")
        )
    }
}

#[derive(Debug)]
pub enum Outcome {
    NotDue,
    Current,
    Skipped(Version),
    Available(Offer),
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    last_check: Option<u64>,
    skipped: Option<Version>,
    // The ETag and its response are one unit: a 304 must not erase a pending release.
    cached: Option<(String, Option<Release>)>,
}

fn timestamp(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

impl State {
    pub fn load(data: &Path) -> Self {
        fs::File::open(data.join("Updates/check.json"))
            .ok()
            .and_then(|f| serde_json::from_reader(io::Read::take(f, 2 * METADATA_LIMIT)).ok())
            .unwrap_or_default()
    }

    fn save(&self, data: &Path) -> Result<()> {
        let save = || -> io::Result<()> {
            let dir = crate::install::updates_dir(data)?;
            let mut file = tempfile::NamedTempFile::new_in(&dir)?;
            serde_json::to_writer(&mut file, self)?;
            file.as_file().sync_all()?;
            file.persist(dir.join("check.json"))?;
            Ok(())
        };
        save().map_err(|e| Error::new(Reason::State, e))
    }

    pub fn due_in(&self, now: SystemTime) -> Duration {
        self.last_check.map_or(Duration::ZERO, |last| {
            let now = timestamp(now);
            // A wall-clock rollback must not permanently disable checks.
            if last > now {
                Duration::ZERO
            } else {
                CHECK_INTERVAL.saturating_sub(Duration::from_secs(now - last))
            }
        })
    }

    pub fn skip(data: &Path, version: Version) -> Result<()> {
        let mut state = Self::load(data);
        state.skipped = Some(version);
        state.save(data)
    }
}

#[derive(Debug)]
pub struct Client {
    current: Version,
    platform: Platform,
    #[cfg(test)]
    fixture: Option<(String, [u8; 32])>,
}

impl Client {
    pub fn new(current: &str, platform: Platform) -> Result<Self> {
        Ok(Self {
            current: Version::parse(current).map_err(|e| Error::new(Reason::Release, e))?,
            platform,
            #[cfg(test)]
            fixture: None,
        })
    }

    pub fn check(
        &self,
        data: &Path,
        state: &mut State,
        now: SystemTime,
        manual: bool,
    ) -> Result<Outcome> {
        if !manual && !state.due_in(now).is_zero() {
            return Ok(Outcome::NotDue);
        }
        state.last_check = Some(timestamp(now));
        // Record attempts, not just successes: offline launches must not retry on every tick.
        state.save(data)?;
        let result = self.check_release(state);
        // Save the new validator even for an unavailable/unsigned candidate.
        state.save(data)?;
        result
    }

    fn check_release(&self, state: &mut State) -> Result<Outcome> {
        let mut body = Vec::new();
        #[cfg(test)]
        let endpoint = self
            .fixture
            .as_ref()
            .map(|(base, _)| format!("{base}/latest"));
        #[cfg(not(test))]
        let endpoint: Option<String> = None;
        let response = self.get(
            endpoint.as_deref().unwrap_or(API),
            state.cached.as_ref().map(|(tag, _)| tag.as_str()),
            &mut body,
            METADATA_LIMIT,
            Duration::from_secs(20),
        )?;
        let release = match response.status {
            304 => state.cached.as_ref().ok_or_else(invalid_release)?.1.clone(),
            200 | 404 => {
                let release = if response.status == 200 {
                    Some(serde_json::from_slice::<Release>(&body).map_err(|_| invalid_release())?)
                } else {
                    None
                };
                state.cached = response.etag.map(|tag| (tag, release.clone()));
                release
            }
            _ => return Err(Error::new(Reason::Network, response.status)),
        };
        let Some(release) = release else {
            return Ok(Outcome::Current);
        };
        let Some(version) = candidate(&release, &self.current)? else {
            return Ok(Outcome::Current);
        };
        if state
            .skipped
            .as_ref()
            .is_some_and(|v| v.cmp_precedence(&version).is_eq())
        {
            return Ok(Outcome::Skipped(version));
        }
        let asset = self.platform.asset(&version);
        let sig_url = self.asset_url(&release, "SHA256SUMS.sig").map_err(|_| {
            Error::new(
                Reason::MissingSignature,
                "release has no usable SHA256SUMS.sig",
            )
        })?;
        let sums_url = self
            .asset_url(&release, "SHA256SUMS")
            .map_err(|_| Error::new(Reason::Checksum, "release has no usable SHA256SUMS"))?;
        let asset_url = self.asset_url(&release, &asset)?;
        let keys = self.keys()?;
        let mut sums = Vec::new();
        let mut signature = Vec::new();
        self.download(&sums_url, &mut sums, 64 * 1024)?;
        self.download(&sig_url, &mut signature, 1024).map_err(|e| {
            if e.reason == Reason::MissingSignature {
                e
            } else {
                Error::new(Reason::Signature, "could not retrieve SHA256SUMS.sig")
            }
        })?;
        let digest = verify::signed_digest(&sums, &signature, &keys, &asset)?;
        Ok(Outcome::Available(Offer {
            version,
            platform: self.platform,
            asset_url,
            digest,
            proof: (sums, signature),
        }))
    }

    fn keys(&self) -> Result<Vec<[u8; 32]>> {
        #[cfg(test)]
        if let Some((_, key)) = self.fixture {
            return Ok(vec![key]);
        }
        verify::trusted_keys()
    }

    fn asset_url(&self, release: &Release, name: &str) -> Result<String> {
        let base = format!("{RELEASES}/download");
        #[cfg(test)]
        let base = self
            .fixture
            .as_ref()
            .map_or(base, |(base, _)| format!("{base}/download"));
        let mut matching = release.assets.iter().filter(|asset| asset.name == name);
        let asset = matching.next().ok_or_else(invalid_release)?;
        let expected = format!("{base}/{}/{name}", release.tag_name);
        if matching.next().is_some()
            || (asset.browser_download_url != expected
                && asset.browser_download_url != expected.replace('+', "%2B"))
        {
            return Err(invalid_release());
        }
        Ok(asset.browser_download_url.clone())
    }

    fn get(
        &self,
        url: &str,
        etag: Option<&str>,
        destination: &mut impl io::Write,
        max_bytes: u64,
        timeout: Duration,
    ) -> Result<Response> {
        let client = HttpsClient {
            timeout,
            ..Default::default()
        };
        let agent = format!("Velocity/{}", self.current);
        let map_error = |error| Error::new(Reason::Network, error);
        #[cfg(test)]
        if self.fixture.is_some() {
            return client
                .get_loopback(url, &agent, etag, destination, max_bytes)
                .map_err(map_error);
        }
        client
            .get(url, &agent, etag, destination, max_bytes)
            .map_err(map_error)
    }

    pub(crate) fn download(
        &self,
        url: &str,
        destination: &mut impl io::Write,
        max_bytes: u64,
    ) -> Result<()> {
        let mut current = url.to_owned();
        for _ in 0..=3 {
            let response = self.get(
                &current,
                None,
                destination,
                max_bytes,
                Duration::from_secs(180),
            )?;
            match response.status {
                200 => return Ok(()),
                301 | 302 | 303 | 307 | 308 => {
                    current = response
                        .location
                        .filter(|location| allowed_redirect(location))
                        .ok_or_else(invalid_release)?;
                }
                404 if url.ends_with("/SHA256SUMS.sig") => {
                    return Err(Error::new(
                        Reason::MissingSignature,
                        "signature download returned 404",
                    ));
                }
                _ => return Err(Error::new(Reason::Network, response.status)),
            }
        }
        Err(invalid_release())
    }
}

fn allowed_redirect(url: &str) -> bool {
    [
        "https://release-assets.githubusercontent.com/",
        "https://objects.githubusercontent.com/",
    ]
    .iter()
    .any(|base| url.starts_with(base))
        && url
            .bytes()
            .all(|b| (0x21..=0x7e).contains(&b) && b != b'\\' && b != b'#')
}

fn invalid_release() -> Error {
    Error::new(
        Reason::Release,
        format_args!("invalid {REPO} release metadata"),
    )
}

fn candidate(release: &Release, current: &Version) -> Result<Option<Version>> {
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let tag = release
        .tag_name
        .strip_prefix('v')
        .filter(|tag| tag.len() <= 80)
        .ok_or_else(invalid_release)?;
    let version = Version::parse(tag).map_err(|_| invalid_release())?;
    Ok((version.pre.is_empty() && version.cmp_precedence(current).is_gt()).then_some(version))
}

#[cfg(test)]
#[path = "release_tests.rs"]
mod tests;
