//! Byte evidence for recovery. The proof table is split at 4 KiB boundaries
//! and at the other revision's EOF. No unchecked bytes in a boundary page
//! are treated as an interrupted write.
use super::*;
use crate::identity::Digest;

pub(super) const PAGE: u64 = 4096;
const TAG: usize = 16;
const IO_CHUNK: usize = 128 * 1024;

#[derive(Debug, Deserialize, Serialize)]
pub(super) enum Original {
    Inline(Vec<u8>),
    // Older journals did not split the old page at the new EOF.
    Blocks { span: u64, hashes: Vec<Fingerprint> },
    Pages(Pages),
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Fingerprint {
    whole: [u8; 32],
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Pages {
    file: JournalPath,
    content: ContentIdentity,
}

impl Pages {
    pub(super) fn path(&self) -> &Path {
        self.file.get()
    }

    pub(super) fn valid_in(&self, root: &Path) -> bool {
        self.path().parent() == Some(root) && staged_name(self.path(), PROOF)
    }

    pub(super) fn read(&self, len: u64, split: u64) -> io::Result<Vec<u8>> {
        let err = || io::Error::other("save proof is missing or changed");
        let expected = tag_count(len, split) * TAG as u64;
        let (file, entry) = stages::open_owned(self.path())
            .map_err(io::Error::other)?
            .ok_or_else(err)?;
        if self.content.bytes != expected || file.metadata()?.len() != expected {
            return Err(err());
        }
        let mut table = Vec::new();
        table
            .try_reserve_exact(usize::try_from(expected).map_err(io::Error::other)?)
            .map_err(io::Error::other)?;
        (&file).take(expected).read_to_end(&mut table)?;
        if table.len() as u64 != expected || blake3::hash(&table).as_bytes() != &self.content.hash {
            return Err(err());
        }
        check_entry(self.path(), &file, entry).map_err(io::Error::other)?;
        Ok(table)
    }
}

fn tag_count(len: u64, split: u64) -> u64 {
    len.div_ceil(PAGE) + u64::from(split < len && !split.is_multiple_of(PAGE))
}

pub(super) struct Scan {
    len: u64,
    split: u64,
    tags: Vec<u8>,
    page: [u8; PAGE as usize],
    offset: u64,
    used: usize,
}

impl Scan {
    pub(super) fn new(len: u64, split: u64) -> io::Result<Self> {
        let mut tags = Vec::new();
        tags.try_reserve_exact(
            usize::try_from(tag_count(len, split) * TAG as u64).map_err(io::Error::other)?,
        )
        .map_err(io::Error::other)?;
        Ok(Self {
            len,
            split,
            tags,
            page: [0; PAGE as usize],
            offset: 0,
            used: 0,
        })
    }

    pub(super) fn save(self, root: &Path) -> io::Result<(Stage, Pages)> {
        if self.offset != self.len || self.used != 0 {
            return Err(io::Error::other(
                "revision length changed while making save proof",
            ));
        }
        let mut stage = Stage::new(root, &preparation_prefix(PROOF), None)?;
        stage.file().write_all(&self.tags)?;
        stage.sync(Durability::Cached)?;
        let pages = Pages {
            file: JournalPath::new(stage.path()),
            content: ContentIdentity {
                bytes: self.tags.len() as u64,
                hash: *blake3::hash(&self.tags).as_bytes(),
            },
        };
        Ok((stage, pages))
    }
}

impl Write for Scan {
    fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
        let size = bytes.len();
        if size as u64 > self.len - self.offset - self.used as u64 {
            return Err(io::Error::other(
                "revision length changed while making save proof",
            ));
        }
        while !bytes.is_empty() {
            if self.used == 0 && self.offset.is_multiple_of(PAGE) {
                let to_split = if self.split > self.offset {
                    self.split - self.offset
                } else {
                    u64::MAX
                };
                let whole = ((bytes.len() as u64 / PAGE).min(to_split / PAGE) * PAGE) as usize;
                if whole != 0 {
                    append_tags(&mut self.tags, &bytes[..whole]);
                    self.offset += whole as u64;
                    bytes = &bytes[whole..];
                    continue;
                }
            }
            let len = (end_of_page(self.offset, self.split).min(self.len) - self.offset) as usize;
            if self.used == 0 && bytes.len() >= len {
                self.tags.extend_from_slice(&tag(&bytes[..len]));
                self.offset += len as u64;
                bytes = &bytes[len..];
            } else {
                let take = (len - self.used).min(bytes.len());
                self.page[self.used..self.used + take].copy_from_slice(&bytes[..take]);
                bytes = &bytes[take..];
                self.used += take;
                if self.used == len {
                    self.tags.extend_from_slice(&tag(&self.page[..len]));
                    self.offset += len as u64;
                    self.used = 0;
                }
            }
        }
        Ok(size)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn append_tags(table: &mut Vec<u8>, bytes: &[u8]) {
    const PAGE_BYTES: usize = PAGE as usize;
    let start = table.len();
    table.resize(start + bytes.len() / PAGE_BYTES * TAG, 0);
    let tags = &mut table[start..];
    let hash = |input: &[u8], output: &mut [u8]| {
        for (page, digest) in input
            .as_chunks::<PAGE_BYTES>()
            .0
            .iter()
            .zip(output.as_chunks_mut::<TAG>().0)
        {
            *digest = tag(page);
        }
    };
    if bytes.len() < 1024 * 1024 {
        hash(bytes, tags);
        return;
    }
    // Bounded, independent work once the encoder hands us a large batch.
    // The tag table always has the same layout regardless of worker count.
    let pages = (bytes.len() / PAGE_BYTES).div_ceil(4);
    let parallel = std::thread::scope(|scope| {
        for (input, output) in bytes
            .chunks(pages * PAGE_BYTES)
            .zip(tags.chunks_mut(pages * TAG))
        {
            if std::thread::Builder::new()
                .spawn_scoped(scope, || hash(input, output))
                .is_err()
            {
                return false;
            }
        }
        true
    });
    if !parallel {
        hash(bytes, tags);
    }
}

fn tag(bytes: &[u8]) -> [u8; TAG] {
    blake3::hash(bytes).as_bytes()[..TAG].try_into().unwrap()
}

fn end_of_page(start: u64, split: u64) -> u64 {
    let end = start.saturating_add(PAGE - start % PAGE);
    if split > start { end.min(split) } else { end }
}

impl Original {
    pub(super) fn capture(
        original: &File,
        len: u64,
        new_len: u64,
        root: &Path,
    ) -> io::Result<(Self, Option<Stage>)> {
        if len <= PAGE {
            let mut bytes = vec![0; len as usize];
            original.read_exact_at(&mut bytes, 0)?;
            return Ok((Self::Inline(bytes), None));
        }
        let mut scan = Scan::new(len, new_len)?;
        read_ranges(original, 0, len, |bytes| scan.write_all(bytes))?;
        let (stage, pages) = scan.save(root)?;
        Ok((Self::Pages(pages), Some(stage)))
    }

    pub(super) fn pages(&self) -> Option<&Pages> {
        match self {
            Self::Pages(pages) => Some(pages),
            _ => None,
        }
    }

    pub(super) fn matches(
        &self,
        target: &File,
        stage: &File,
        actual: u64,
        old: u64,
        new: u64,
    ) -> io::Result<bool> {
        if actual < old && actual != new {
            return Ok(false);
        }
        let redo = Revision::file(stage, 0, new);
        let table;
        let undo = match self {
            Self::Inline(bytes) if bytes.len() as u64 == old && old <= PAGE => Revision {
                len: old,
                evidence: Evidence::Memory(bytes),
            },
            Self::Pages(pages) => {
                table = pages.read(old, new)?;
                Revision::pages(&table, old, new)
            }
            Self::Blocks { span, hashes }
                if *span >= PAGE && old.div_ceil(*span) == hashes.len() as u64 =>
            {
                return matches_legacy(target, stage, actual, old, new, *span, hashes);
            }
            _ => return Ok(false),
        };
        Ok(!matches!(
            verify(target, actual, undo, Some(redo))?,
            Overwrite::Foreign
        ))
    }
}

fn matches_legacy(
    target: &File,
    stage: &File,
    actual: u64,
    old: u64,
    new: u64,
    span: u64,
    hashes: &[Fingerprint],
) -> io::Result<bool> {
    if actual > old.max(new) {
        return Ok(false);
    }
    let mut offset = 0;
    while offset < actual {
        let count = (actual - offset).min(span);
        let was = old.saturating_sub(offset).min(span);
        if count <= new.saturating_sub(offset)
            && checksum_range(target, offset, count)? == checksum_range(stage, offset, count)?
        {
            offset += count;
            continue;
        }
        if was > count
            || (was != 0
                && checksum_range(target, offset, was)?.hash
                    != hashes[(offset / span) as usize].whole)
            || !zero_range(target, offset + was, count - was)?
        {
            return Ok(false);
        }
        offset += count;
    }
    Ok(true)
}

enum Evidence<'a> {
    File(&'a File, u64),
    Memory(&'a [u8]),
    Pages(&'a [u8], u64),
}

pub(super) struct Revision<'a> {
    len: u64,
    evidence: Evidence<'a>,
}

impl<'a> Revision<'a> {
    pub(super) fn file(file: &'a File, start: u64, len: u64) -> Self {
        Self {
            len,
            evidence: Evidence::File(file, start),
        }
    }

    pub(super) fn pages(table: &'a [u8], len: u64, split: u64) -> Self {
        Self {
            len,
            evidence: Evidence::Pages(table, split),
        }
    }

    // Fill expected bytes when held; compare against a tag otherwise.
    // None for expected bytes means the remainder of this piece is unproven.
    fn compare(&self, offset: u64, actual: &[u8], expected: &mut [u8]) -> io::Result<(bool, bool)> {
        if offset >= self.len {
            expected.fill(0);
        } else {
            match self.evidence {
                Evidence::File(file, start) => file.read_exact_at(expected, start + offset)?,
                Evidence::Memory(bytes) => {
                    let from = offset as usize;
                    expected.copy_from_slice(&bytes[from..from + actual.len()]);
                }
                Evidence::Pages(table, split) => {
                    if end_of_page(offset, split).min(self.len) != offset + actual.len() as u64 {
                        return Ok((false, false));
                    }
                    let extra = u64::from(split <= offset && !split.is_multiple_of(PAGE));
                    let at = (offset / PAGE + extra) as usize * TAG;
                    return Ok((
                        table.get(at..at + TAG) == Some(tag(actual).as_slice()),
                        false,
                    ));
                }
            }
        }
        Ok((actual == expected, true))
    }
}

/// `through` bounds the range changed relative to the rollback image.
pub(super) enum Overwrite {
    Original,
    Interrupted { through: u64 },
    Foreign,
}

pub(super) fn verify(
    target: &File,
    actual: u64,
    old: Revision<'_>,
    new: Option<Revision<'_>>,
) -> io::Result<Overwrite> {
    let new_len = new.as_ref().map_or(old.len, |r| r.len);
    if actual < old.len.min(new_len) || actual > old.len.max(new_len) {
        return Ok(Overwrite::Foreign);
    }
    let mut observed = vec![0; actual.min(IO_CHUNK as u64) as usize];
    let mut before = [0; PAGE as usize];
    let mut after = [0; PAGE as usize];
    let mut through = 0;
    let mut base = 0;
    while base < actual {
        let size = (actual - base).min(IO_CHUNK as u64) as usize;
        target.read_exact_at(&mut observed[..size], base)?;
        let mut at = 0;
        while at < size {
            let offset = base + at as u64;
            let end = end_of_page(offset, old.len)
                .min(end_of_page(offset, new_len))
                .min(actual);
            let count = (end - offset) as usize;
            let part = &observed[at..at + count];
            at += count;
            let (old_match, old_bytes) = old.compare(offset, part, &mut before[..count])?;
            if old_match {
                continue;
            }
            let Some(new) = new.as_ref().filter(|new| offset < new.len) else {
                return Ok(Overwrite::Foreign);
            };
            let (new_match, new_bytes) = new.compare(offset, part, &mut after[..count])?;
            if !new_match
                && !(old_bytes
                    && new_bytes
                    && one_transition(part, &before[..count], &after[..count]))
            {
                return Ok(Overwrite::Foreign);
            }
            through = end;
        }
        base += size as u64;
    }
    Ok(if through == 0 && actual == old.len {
        Overwrite::Original
    } else {
        Overwrite::Interrupted { through }
    })
}

fn one_transition(actual: &[u8], old: &[u8], new: &[u8]) -> bool {
    // Complete pages can reach storage out of order. Inside a page, accept a
    // short write (or short rollback) only if both sides are known byte for byte.
    let first_diff = |expected: &[u8]| actual.iter().zip(expected).position(|(a, b)| a != b);
    first_diff(old).is_some_and(|at| actual[at..] == new[at..])
        || first_diff(new).is_some_and(|at| actual[at..] == old[at..])
}

pub(super) fn is_prefix(target: &File, stage: &File, actual: u64, new: u64) -> io::Result<bool> {
    Ok(actual <= new && checksum(target, actual)? == checksum(stage, actual)?)
}

fn read_ranges(
    file: &File,
    start: u64,
    len: u64,
    mut read: impl FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<()> {
    let mut bytes = vec![0; len.min(IO_CHUNK as u64) as usize];
    let mut offset = 0;
    while offset < len {
        let count = (len - offset).min(bytes.len() as u64) as usize;
        file.read_exact_at(&mut bytes[..count], start + offset)?;
        read(&bytes[..count])?;
        offset += count as u64;
    }
    Ok(())
}

fn zero_range(file: &File, start: u64, len: u64) -> io::Result<bool> {
    let mut zero = true;
    read_ranges(file, start, len, |bytes| {
        zero &= bytes.iter().all(|&b| b == 0);
        Ok(())
    })?;
    Ok(zero)
}

pub(super) fn checksum(file: &File, len: u64) -> io::Result<ContentIdentity> {
    checksum_range(file, 0, len)
}

pub(super) fn checksum_range(file: &File, start: u64, len: u64) -> io::Result<ContentIdentity> {
    let mut digest = Digest::default();
    read_ranges(file, start, len, |bytes| {
        digest.update(bytes);
        Ok(())
    })?;
    Ok(digest.identity())
}
