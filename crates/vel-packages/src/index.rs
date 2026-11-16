use crate::{
    Package, PackageManager, Result, Version,
    https::{self, HttpsClient},
    invalid, validate_mutable_name,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read},
    path::Path,
};

/// Minimal Velocity index schema. There is deliberately no bundled/default index.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Index {
    /// Schema revision; currently exactly `1`.
    pub schema_version: u32,
    /// One or more release versions; highest compatible SemVer wins for each name.
    pub packages: Vec<Release>,
}
/// One downloadable declarative package release, authenticated by its index-provided digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    /// Package name, without archive extension.
    pub name: String,
    /// Must match the archive's `package-metadata.json` version when installed.
    pub version: Version,
    /// HTTPS archive URL. Redirects must also use HTTPS.
    pub url: String,
    /// SHA-256 of the entire archive, as 64 hexadecimal characters.
    pub sha256: String,
}
impl Release {
    pub(crate) fn validate(&self) -> Result<()> {
        validate_mutable_name(&self.name)?;
        https::validate_url(&self.url)?;
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid("index SHA-256 must contain 64 hex digits"));
        }
        Ok(())
    }
}
/// A newer release, or a release for an installed package whose local version is unknown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upgrade {
    /// Current effective metadata version (`None` means unknown, not version zero).
    pub installed_version: Option<Version>,
    /// Highest indexed release. No download or installation has happened yet.
    pub available: Release,
}
impl Index {
    /// Parses and validates the entire index. Unknown fields and schema versions are errors.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let index: Self = serde_json::from_slice(bytes).map_err(invalid)?;
        index.validate()?;
        Ok(index)
    }
    fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            return Err(invalid("unsupported index schema_version"));
        }
        if self.packages.len() > 10_000 {
            return Err(invalid("too many index entries"));
        }
        let mut entries = std::collections::BTreeSet::new();
        for release in &self.packages {
            release.validate()?;
            if !entries.insert((&release.name, &release.version)) {
                return Err(invalid("duplicate index release"));
            }
        }
        Ok(())
    }
    /// Computes available upgrades without network access. Stable installs skip prereleases.
    /// Disabled packages are included so the UI can show updates without enabling them.
    pub fn upgrades(&self, installed: &[Package]) -> Result<Vec<Upgrade>> {
        self.validate()?;
        let mut upgrades = Vec::new();
        for package in installed {
            let latest = self
                .packages
                .iter()
                .filter(|r| r.name == package.name)
                .filter(|r| {
                    r.version.pre.is_empty()
                        || package.version.as_ref().is_some_and(|v| !v.pre.is_empty())
                })
                .max_by(|a, b| a.version.cmp_precedence(&b.version));
            if let Some(release) = latest
                && package
                    .version
                    .as_ref()
                    .is_none_or(|v| release.version.cmp_precedence(v).is_gt())
            {
                upgrades.push(Upgrade {
                    installed_version: package.version.clone(),
                    available: release.clone(),
                });
            }
        }
        upgrades.sort_by(|a, b| a.available.name.cmp(&b.available.name));
        Ok(upgrades)
    }
}
impl PackageManager {
    /// Fetches an explicitly configured index and checks versions. With no index this is a
    /// no-op, including no filesystem discovery, process spawn or network access.
    pub fn check_upgrades(&self, client: &HttpsClient) -> Result<Vec<Upgrade>> {
        let Some(url) = self.index_url() else {
            return Ok(Vec::new());
        };
        let temp = tempfile::NamedTempFile::new()?;
        client.download(url, temp.path(), 1024 * 1024)?;
        let index = Index::parse(&fs::read(temp.path())?)?;
        index.upgrades(&self.list()?)
    }
}
pub(crate) fn sha256(path: &Path) -> Result<String> {
    struct Hasher(Sha256);
    impl io::Write for Hasher {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut hasher = Hasher(Sha256::new());
    io::copy(
        &mut fs::File::open(path)?.take(super::archive::MAX_ARCHIVE + 1),
        &mut hasher,
    )?;
    Ok(hasher
        .0
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
