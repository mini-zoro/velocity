//! Hash raw bytes in the existing IO pass, never by scanning a live editor buffer.
use crate::ContentIdentity;
use std::io::{self, Read, Write};
#[derive(Default)]
pub(crate) struct Digest {
    hasher: blake3::Hasher,
    bytes: u64,
}
impl Digest {
    pub(crate) fn update(&mut self, bytes: &[u8]) {
        self.hasher.update(bytes);
        self.bytes += bytes.len() as u64;
    }
    pub fn identity(&self) -> ContentIdentity {
        ContentIdentity {
            bytes: self.bytes,
            hash: *self.hasher.finalize().as_bytes(),
        }
    }
}
pub(crate) struct DigestReader<R> {
    pub input: R,
    pub digest: Digest,
    /// The first bytes read, as much as the decode decision inspects.
    pub prefix: Vec<u8>,
}
impl<R> DigestReader<R> {
    pub fn new(input: R) -> Self {
        Self {
            input,
            digest: Digest::default(),
            prefix: Vec::new(),
        }
    }
}
impl<R: Read> Read for DigestReader<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let n = self.input.read(bytes)?;
        self.digest.update(&bytes[..n]);
        let room = crate::decoding::PREFIX_BYTES.saturating_sub(self.prefix.len());
        self.prefix.extend_from_slice(&bytes[..n.min(room)]);
        Ok(n)
    }
}
pub(crate) struct DigestWriter<'a, W> {
    pub output: W,
    pub digest: &'a mut Digest,
}
impl<W: Write> Write for DigestWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let n = self.output.write(bytes)?;
        self.digest.update(&bytes[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

/// Reads a regular document's raw revision without decoding or normalizing it.
/// Saves use the same identity as completed loads and writes. A missing target
/// has no revision; special files are rejected before any potentially blocking read.
pub fn content_identity(path: &std::path::Path) -> Result<Option<ContentIdentity>, String> {
    if crate::regular::metadata(path, true)?.is_none() {
        return Ok(None);
    }
    let cancel = crate::progressive::current_cancellation();
    let mut reader = DigestReader::new(crate::progressive::CheckedReader {
        input: crate::regular::read(path)?,
        cancel: cancel.as_ref(),
    });
    let mut bytes = vec![0; 128 * 1024];
    loop {
        let n = reader
            .read(&mut bytes)
            .map_err(|e| crate::regular::error(path, e))?;
        if n == 0 {
            break;
        }
    }
    Ok(Some(reader.digest.identity()))
}
