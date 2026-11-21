//! Selectively read a replaceable system font collection without mapping it.
use objc2_core_text::{CTFont, CTFontTableOptions};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
};
use swash::{FontRef, StringId};
use vel_platform::fonts::is_mobile_asset_path;

const MAX_FACE_BYTES: usize = 128 * 1024 * 1024;
const NAME: [u8; 4] = *b"name";
const SIGNATURE: [[u8; 4]; 3] = [NAME, *b"cmap", *b"head"];

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid font collection")
}

fn read_at(file: &mut File, offset: u32, into: &mut [u8]) -> io::Result<()> {
    file.seek(SeekFrom::Start(u64::from(offset)))?;
    file.read_exact(into)
}

pub(super) fn mobile_asset_face(
    path: &Path,
    name: &str,
    native: Option<&CTFont>,
    outlines: bool,
) -> Result<Option<Vec<u8>>, String> {
    if !is_mobile_asset_path(path) {
        return Ok(None);
    }
    fn load(
        path: &Path,
        name: &str,
        native: Option<&CTFont>,
        outlines: bool,
    ) -> io::Result<Option<Vec<u8>>> {
        let canonical = path.canonicalize()?;
        if !is_mobile_asset_path(&canonical) {
            return Ok(None);
        }
        let mut file = File::open(canonical)?;
        let before = file.metadata()?;
        let mut header = [0; 12];
        file.read_exact(&mut header)?;
        if &header[..4] != b"ttcf" {
            return Ok(None);
        }
        let count = u32::from_be_bytes(header[8..12].try_into().unwrap());
        if !(1..=256).contains(&count) {
            return Err(invalid());
        }
        let mut offsets = vec![0; count as usize * 4];
        file.read_exact(&mut offsets)?;
        let mut dirs = Vec::new();
        let mut selected = None;
        for offset in offsets.as_chunks::<4>().0 {
            let dir = Directory::read(&mut file, u32::from_be_bytes(*offset))?;
            let names = dir.copy(&mut file, before.len(), |tag| tag == NAME)?;
            if FontRef::from_index(&names, 0).is_some_and(|font| {
                font.localized_strings()
                    .any(|s| s.id() == StringId::PostScript && s.to_string() == name)
            }) {
                selected = Some(dir);
                break;
            }
            dirs.push(dir);
        }
        let selected = selected.or_else(|| {
            // Private cascade names need not occur in the collection's name table.
            // Use the same three unmodified identity tables as the full-file path.
            let native = native?;
            let tables = SIGNATURE.map(|tag| {
                // SAFETY: this is a retained immutable CoreText font.
                unsafe { native.table(u32::from_be_bytes(tag), CTFontTableOptions::empty()) }
                    .map(|table| table.to_vec())
            });
            let mut matching = dirs.into_iter().filter(|dir| {
                let Ok(data) = dir.copy(&mut file, before.len(), |tag| SIGNATURE.contains(&tag))
                else {
                    return false;
                };
                let Some(font) = FontRef::from_index(&data, 0) else {
                    return false;
                };
                SIGNATURE.iter().zip(&tables).all(|(tag, table)| {
                    table.as_ref().is_some_and(|table| {
                        font.table(u32::from_be_bytes(*tag)) == Some(table.as_slice())
                    })
                })
            });
            let face = matching.next()?;
            matching.next().is_none().then_some(face)
        });
        let face = selected.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("exact face {name} unavailable"),
            )
        })?;
        let bytes = face.copy(&mut file, before.len(), |tag| {
            outlines || !matches!(&tag, b"CFF " | b"CFF2" | b"glyf" | b"loca")
        })?;
        let after = file.metadata()?;
        if before.len() != after.len() || before.modified()? != after.modified()? {
            return Err(io::Error::other("font collection changed while reading"));
        }
        Ok(Some(bytes))
    }
    load(path, name, native, outlines).map_err(|e| format!("{}: {e}", path.display()))
}

struct Directory {
    flavor: [u8; 4],
    records: Vec<u8>,
}

impl Directory {
    fn read(file: &mut File, at: u32) -> io::Result<Self> {
        let mut header = [0; 12];
        read_at(file, at, &mut header)?;
        let count = u16::from_be_bytes(header[4..6].try_into().unwrap());
        let flavor = header[..4].try_into().unwrap();
        if count == 0 || count > 4095 || !matches!(&flavor, b"OTTO" | b"true" | b"\0\x01\0\0") {
            return Err(invalid());
        }
        let mut records = vec![0; 16 * usize::from(count)];
        file.read_exact(&mut records)?;
        Ok(Self { flavor, records })
    }

    fn copy(
        &self,
        file: &mut File,
        file_len: u64,
        keep: impl Fn([u8; 4]) -> bool,
    ) -> io::Result<Vec<u8>> {
        let tables = || {
            self.records
                .as_chunks::<16>()
                .0
                .iter()
                .filter(|r| keep(r[..4].try_into().unwrap()))
        };
        let count = tables().count() as u16;
        let directory_end = 12 + 16 * usize::from(count);
        let mut size = directory_end;
        for r in tables() {
            let offset = u32::from_be_bytes(r[8..12].try_into().unwrap());
            let len = u32::from_be_bytes(r[12..16].try_into().unwrap());
            if u64::from(offset) + u64::from(len) > file_len || len as usize > MAX_FACE_BYTES {
                return Err(invalid());
            }
            size = size
                .checked_add((len as usize).next_multiple_of(4))
                .filter(|&size| size <= MAX_FACE_BYTES)
                .ok_or_else(invalid)?;
        }
        // Swash only needs an internal SFNT view: keep the exact table bytes
        // (including outlines and hinting), rebase offsets and ignore the
        // whole-file checksum adjustment. This view is never written to disk.
        let mut bytes = vec![0; size];
        bytes[..4].copy_from_slice(&self.flavor);
        bytes[4..6].copy_from_slice(&count.to_be_bytes());
        if count != 0 {
            let shift = count.ilog2();
            let search = 16u16 << shift;
            bytes[6..8].copy_from_slice(&search.to_be_bytes());
            bytes[8..10].copy_from_slice(&(shift as u16).to_be_bytes());
            bytes[10..12].copy_from_slice(&(count * 16 - search).to_be_bytes());
        }
        let mut dst = directory_end;
        for (r, out) in tables().zip((12..directory_end).step_by(16)) {
            let offset = u32::from_be_bytes(r[8..12].try_into().unwrap());
            let len = u32::from_be_bytes(r[12..16].try_into().unwrap()) as usize;
            bytes[out..out + 16].copy_from_slice(r);
            bytes[out + 8..out + 12].copy_from_slice(&(dst as u32).to_be_bytes());
            read_at(file, offset, &mut bytes[dst..dst + len])?;
            dst += len.next_multiple_of(4);
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn mobile_asset_face_preserves_shaping_and_outline_tables() {
        let Ok(assets) = std::fs::read_dir("/System/Library/AssetsV2") else {
            return;
        };
        let Some(path) = assets
            .flatten()
            .filter(|dir| {
                dir.file_name()
                    .to_string_lossy()
                    .starts_with("com_apple_MobileAsset_Font")
            })
            .filter_map(|dir| std::fs::read_dir(dir.path()).ok())
            .flatten()
            .flatten()
            .map(|dir| dir.path().join("AssetData/PingFang.ttc"))
            .find(|file| file.is_file())
        else {
            return; // The OS may not have installed this optional asset.
        };
        let name = "PingFangSC-Regular";
        let data = mobile_asset_face(&path, name, None, true)
            .unwrap()
            .expect("MobileAsset TTC");
        let metrics = mobile_asset_face(&path, name, None, false)
            .unwrap()
            .expect("MobileAsset TTC");
        let original = std::fs::read(path).unwrap();
        let snapshot = FontRef::from_index(&data, 0).unwrap();
        let metrics = FontRef::from_index(&metrics, 0).unwrap();
        let exact = swash::FontDataRef::new(&original)
            .unwrap()
            .fonts()
            .find(|f| {
                f.table(u32::from_be_bytes(NAME)) == snapshot.table(u32::from_be_bytes(NAME))
                    && f.table(u32::from_be_bytes(*b"cmap"))
                        == snapshot.table(u32::from_be_bytes(*b"cmap"))
            })
            .unwrap();
        for tag in [
            b"head", b"cmap", b"hmtx", b"GPOS", b"GSUB", b"CFF ", b"glyf",
        ] {
            let tag = u32::from_be_bytes(*tag);
            assert_eq!(exact.table(tag), snapshot.table(tag));
        }
        assert_eq!(snapshot.charmap().map('你'), exact.charmap().map('你'));
        assert_eq!(metrics.charmap().map('你'), exact.charmap().map('你'));
        assert!(metrics.table(u32::from_be_bytes(*b"CFF ")).is_none());
        assert!(metrics.table(u32::from_be_bytes(*b"glyf")).is_none());
        assert!(data.len() < original.len());
    }

    #[test]
    fn rebased_table_is_owned_and_a_truncated_read_fails() {
        let path = std::env::temp_dir().join(format!("vel-font-snapshot-{}", std::process::id()));
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        std::fs::remove_file(path).unwrap();
        file.write_all(b"OTTO\0\x01\0\0\0\0\0\0TEST\0\0\0\0\0\0\0\x20\0\0\0\x04\0\0\0\0data")
            .unwrap();
        let dir = Directory::read(&mut file, 0).unwrap();
        let data = dir.copy(&mut file, 36, |_| true).unwrap();
        file.set_len(32).unwrap();
        assert!(dir.copy(&mut file, 36, |_| true).is_err());
        let face = FontRef::from_index(&data, 0).unwrap();
        assert_eq!(
            face.table(u32::from_be_bytes(*b"TEST")),
            Some(b"data".as_slice())
        );
    }
}
