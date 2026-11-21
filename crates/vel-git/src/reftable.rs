//! Read-only reference lookup, following <https://git-scm.com/docs/reftable>.
//! Objects, the index and worktree status still belong to gix.
use gix::hash::ObjectId;
use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub(crate) struct Store {
    worktree: PathBuf,
    common: PathBuf,
}

#[derive(Debug)]
enum Ref {
    Deleted,
    Object(ObjectId),
    Symbolic(Vec<u8>),
}

impl Store {
    pub(crate) fn detect(repo: &gix::Repository) -> Option<Self> {
        let config = repo.config_snapshot();
        let storage = config.string("extensions.refStorage")?;
        storage.eq_ignore_ascii_case(b"reftable").then(|| Self {
            worktree: repo.git_dir().join("reftable"),
            common: repo.common_dir().join("reftable"),
        })
    }

    pub(crate) fn head(&self) -> io::Result<(super::Head, Option<ObjectId>)> {
        self.with_retry(|worktree, common| {
            match lookup(worktree, b"HEAD")?.ok_or_else(invalid)? {
                Ref::Object(id) => Ok((super::Head::Detached(id.to_string()), Some(id))),
                Ref::Symbolic(name) => {
                    let id = resolve(worktree, common, &name)?;
                    let short = name.strip_prefix(b"refs/heads/").unwrap_or(&name);
                    Ok((
                        super::Head::Branch {
                            name: String::from_utf8_lossy(short).into_owned(),
                            unborn: id.is_none(),
                        },
                        id,
                    ))
                }
                Ref::Deleted => Err(invalid()),
            }
        })
    }

    pub(crate) fn resolve(&self, name: &[u8]) -> io::Result<Option<ObjectId>> {
        self.with_retry(|worktree, common| resolve(worktree, common, name))
    }

    fn with_retry<T>(
        &self,
        read: impl Fn(&Stack<'_>, &Stack<'_>) -> io::Result<T>,
    ) -> io::Result<T> {
        let attempt = || {
            let common = Stack::read(&self.common)?;
            if self.worktree == self.common {
                read(&common, &common)
            } else {
                read(&Stack::read(&self.worktree)?, &common)
            }
        };
        match attempt() {
            // Git may replace the stack and unlink compacted tables after the list read.
            Err(e) if e.kind() == io::ErrorKind::NotFound => attempt(),
            result => result,
        }
    }
}

fn resolve(worktree: &Stack<'_>, common: &Stack<'_>, name: &[u8]) -> io::Result<Option<ObjectId>> {
    let mut name = name.to_vec();
    for _ in 0..16 {
        let worktree_ref = !name.starts_with(b"refs/")
            || [
                b"refs/bisect/".as_slice(),
                b"refs/worktree/",
                b"refs/rewritten/",
            ]
            .iter()
            .any(|prefix| name.starts_with(prefix));
        match lookup(if worktree_ref { worktree } else { common }, &name)? {
            Some(Ref::Object(id)) => return Ok(Some(id)),
            Some(Ref::Symbolic(next)) => name = next,
            Some(Ref::Deleted) | None => return Ok(None),
        }
    }
    Err(invalid())
}

struct Stack<'a> {
    directory: &'a Path,
    list: String,
}
impl<'a> Stack<'a> {
    fn read(directory: &'a Path) -> io::Result<Self> {
        const LIMIT: u64 = 2 * 1024 * 1024;
        let mut list = String::new();
        regular(&directory.join("tables.list"))?
            .take(LIMIT + 1)
            .read_to_string(&mut list)?;
        if list.len() as u64 > LIMIT {
            return Err(invalid());
        }
        Ok(Self { directory, list })
    }
}

fn lookup(stack: &Stack<'_>, key: &[u8]) -> io::Result<Option<Ref>> {
    for name in stack.list.lines().rev() {
        if !name.ends_with(".ref")
            || name.len() > 255
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-'))
        {
            return Err(invalid());
        }
        if let Some(value) = lookup_table(&mut regular(&stack.directory.join(name))?, key)? {
            return Ok(Some(value)); // Includes tombstones; never fall back to an older ref.
        }
    }
    Ok(None)
}

fn regular(path: &Path) -> io::Result<File> {
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(invalid());
    }
    File::open(path)
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid git reftable")
}
fn u24(bytes: &[u8]) -> usize {
    u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]) as usize
}
fn be64(bytes: &[u8]) -> u64 {
    u64::from_be_bytes(bytes[..8].try_into().expect("eight bytes"))
}
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn lookup_table(file: &mut File, key: &[u8]) -> io::Result<Option<Ref>> {
    let mut header = [0; 28];
    file.read_exact(&mut header[..24])?;
    let (head_len, hash_len) = match (&header[..4], header[4]) {
        (b"REFT", 1) => (24usize, 20),
        (b"REFT", 2) => {
            file.read_exact(&mut header[24..])?;
            match &header[24..] {
                b"sha1" => (28, 20),
                b"s256" => (28, 32),
                _ => return Err(invalid()),
            }
        }
        _ => return Err(invalid()),
    };
    let footer_len = head_len + 44;
    let size = file.metadata()?.len();
    let data_end = size
        .checked_sub(footer_len as u64)
        .filter(|end| *end >= head_len as u64)
        .ok_or_else(invalid)?;
    let mut footer = [0; 72];
    file.seek(SeekFrom::Start(data_end))?;
    file.read_exact(&mut footer[..footer_len])?;
    let crc = u32::from_be_bytes(footer[footer_len - 4..footer_len].try_into().expect("crc"));
    if footer[..head_len] != header[..head_len] || crc32(&footer[..footer_len - 4]) != crc {
        return Err(invalid());
    }
    let refs_end = [
        be64(&footer[head_len..]), // Ref index root (there may be lower index blocks earlier).
        be64(&footer[head_len + 8..]) >> 5, // Object blocks.
        be64(&footer[head_len + 24..]), // Log blocks.
        data_end,
    ]
    .into_iter()
    .filter(|p| *p != 0)
    .min()
    .unwrap_or(data_end);
    let alignment = u24(&header[5..8]) as u64;
    let mut block = Vec::new();
    let mut offset = 0u64;
    let mut prefix = head_len;
    while offset + prefix as u64 + 4 <= refs_end {
        file.seek(SeekFrom::Start(offset + prefix as u64))?;
        let mut kind = [0; 4];
        file.read_exact(&mut kind)?;
        match kind[0] {
            b'r' => {}
            b'i' | b'o' | b'g' => break,
            _ => return Err(invalid()),
        }
        let len = u24(&kind[1..]) as u64;
        if len < (prefix + 9) as u64
            || offset + len > refs_end
            || (alignment != 0 && len > alignment)
        {
            return Err(invalid());
        }
        block.resize(len as usize - prefix - 4, 0);
        file.read_exact(&mut block)?;
        let n = block.len();
        let restarts = u16::from_be_bytes([block[n - 2], block[n - 1]]) as usize;
        let end = n
            .checked_sub(2 + 3 * restarts)
            .filter(|end| *end != 0 && restarts != 0)
            .ok_or_else(invalid)?;
        if let Some(value) = lookup_records(&block[..end], key, hash_len)? {
            return Ok(Some(value));
        }
        offset += if alignment == 0 { len } else { alignment };
        prefix = 0;
    }
    Ok(None)
}

fn take<'a>(input: &mut &'a [u8], count: usize) -> io::Result<&'a [u8]> {
    let (prefix, tail) = input.split_at_checked(count).ok_or_else(invalid)?;
    *input = tail;
    Ok(prefix)
}
fn varint(input: &mut &[u8]) -> io::Result<usize> {
    let mut byte = take(input, 1)?[0];
    let mut value = usize::from(byte & 127);
    while byte & 128 != 0 {
        byte = take(input, 1)?[0];
        value = value
            .checked_add(1)
            .and_then(|v| v.checked_mul(128))
            .and_then(|v| v.checked_add(usize::from(byte & 127)))
            .ok_or_else(invalid)?;
    }
    Ok(value)
}
fn lookup_records(mut input: &[u8], key: &[u8], hash_len: usize) -> io::Result<Option<Ref>> {
    let mut name = Vec::new();
    while !input.is_empty() {
        let prefix = varint(&mut input)?;
        let suffix_and_kind = varint(&mut input)?;
        if prefix > name.len() {
            return Err(invalid());
        }
        name.truncate(prefix);
        name.extend_from_slice(take(&mut input, suffix_and_kind >> 3)?);
        let _update_delta = varint(&mut input)?;
        if name.as_slice() > key {
            return Ok(None);
        }
        let found = name == key;
        let value = match suffix_and_kind & 7 {
            0 => found.then_some(Ref::Deleted),
            kind @ (1 | 2) => {
                let bytes = take(&mut input, kind * hash_len)?;
                if found {
                    Some(Ref::Object(
                        ObjectId::try_from(&bytes[..hash_len]).map_err(|_| invalid())?,
                    ))
                } else {
                    None
                }
            }
            3 => {
                let length = varint(&mut input)?;
                let target = take(&mut input, length)?;
                found.then(|| Ref::Symbolic(target.to_vec()))
            }
            _ => return Err(invalid()),
        };
        if found {
            return Ok(value);
        }
    }
    Ok(None)
}
