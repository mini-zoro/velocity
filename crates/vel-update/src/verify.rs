use crate::{Error, Reason, Result};
use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};
use std::io::{self, Read};

// To rotate, first ship a bridge release trusting both keys, signed by the old key.
// An empty list fails closed. This file must contain public keys only.
const TRUSTED_PUBLIC_KEYS: &str = include_str!("public-keys.hex");

pub(crate) fn trusted_keys() -> Result<Vec<[u8; 32]>> {
    let keys: Option<Vec<_>> = TRUSTED_PUBLIC_KEYS
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(decode)
        .collect();
    keys.filter(|keys| !keys.is_empty())
        .ok_or_else(|| Error::new(Reason::SigningKey, "no valid update trust keys configured"))
}

pub(crate) fn decode<const N: usize>(hex: &str) -> Option<[u8; N]> {
    if hex.len() != N * 2 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = [0; N];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(bytes)
}

pub(crate) fn signed_digest(
    sums: &[u8],
    signature: &[u8],
    keys: &[[u8; 32]],
    asset: &str,
) -> Result<[u8; 32]> {
    if signature.is_empty() {
        return Err(Error::new(Reason::MissingSignature, "empty SHA256SUMS.sig"));
    }
    let invalid = || Error::new(Reason::Signature, "invalid SHA256SUMS.sig");
    let sig = Signature::from_slice(signature).map_err(|_| invalid())?;
    if !keys.iter().any(|bytes| {
        VerifyingKey::from_bytes(bytes).is_ok_and(|key| key.verify_strict(sums, &sig).is_ok())
    }) {
        return Err(invalid());
    }
    let checksum = || {
        Error::new(
            Reason::Checksum,
            "missing, malformed, or duplicate checksum",
        )
    };
    let mut result = None;
    for row in std::str::from_utf8(sums).map_err(|_| checksum())?.lines() {
        // Release CI produces the standard shasum format: two spaces, or space and '*'.
        let (hash, name) = row.split_once([' ', '\t']).ok_or_else(checksum)?;
        let name = name.strip_prefix([' ', '*']).ok_or_else(checksum)?;
        let digest = decode(hash).ok_or_else(checksum)?;
        if name == asset && result.replace(digest).is_some() {
            return Err(checksum());
        }
    }
    result.ok_or_else(checksum)
}

pub(crate) fn digest(mut input: impl Read) -> io::Result<[u8; 32]> {
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        match input.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => hash.update(&buffer[..count]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(hash.finalize().into())
}

pub(crate) fn check_digest(input: impl Read, expected: &[u8; 32]) -> Result<()> {
    if digest(input)? != *expected {
        return Err(Error::new(Reason::Checksum, "archive SHA-256 mismatch"));
    }
    Ok(())
}
