//! Bounded two-block pipeline for large encodes. The writer must join before
//! the caller may publish its stage or overwrite a failed write with rollback.
use crate::{ContentIdentity, Format, identity, write_encoded};
use std::{
    io::{self, Write},
    sync::mpsc,
};
use vel_text::Text;

const BLOCK: usize = 16 * 1024 * 1024;
const ALIGN: usize = 16 * 1024;

pub(super) fn encode(
    output: &mut (dyn Write + Send),
    text: &Text,
    format: &Format,
    codec: &'static encoding_rs::Encoding,
) -> Result<ContentIdentity, String> {
    std::thread::scope(|scope| {
        let (submit, blocks) = mpsc::sync_channel::<Chunk>(1);
        let (reuse, recycled) = mpsc::channel::<Chunk>();
        let disk = std::thread::Builder::new()
            .name("velocity-save-stream".into())
            .spawn_scoped(scope, move || -> io::Result<()> {
                for block in blocks {
                    output.write_all(block.bytes())?;
                    let _ = reuse.send(block);
                }
                output.flush()
            })
            .map_err(|e| e.to_string())?;
        let mut writer = Producer {
            submit,
            recycled,
            buffer: Some(Chunk::new()),
            spare: Some(Chunk::new()),
            digest: identity::Digest::default(),
        };
        let result = write_encoded(&mut writer, text, format, codec);
        let identity = writer.digest.identity();
        drop(writer);
        disk.join()
            .map_err(|_| "save writer stopped".to_owned())?
            .map_err(|e| e.to_string())?;
        result.map(|()| identity)
    })
}

struct Chunk {
    storage: Vec<u8>,
    start: usize,
    used: usize,
}

impl Chunk {
    fn new() -> Self {
        let storage = vec![0; BLOCK + ALIGN];
        let start = storage.as_ptr().align_offset(ALIGN);
        Self {
            storage,
            start,
            used: 0,
        }
    }
    fn bytes(&self) -> &[u8] {
        &self.storage[self.start..self.start + self.used]
    }
}

struct Producer {
    submit: mpsc::SyncSender<Chunk>,
    recycled: mpsc::Receiver<Chunk>,
    buffer: Option<Chunk>,
    spare: Option<Chunk>,
    digest: identity::Digest,
}

impl Producer {
    fn buffer(&mut self) -> io::Result<&mut Chunk> {
        if self.buffer.is_none() {
            let mut next = match self.spare.take() {
                Some(spare) => spare,
                None => self.recycled.recv().map_err(io::Error::other)?,
            };
            next.used = 0;
            self.buffer = Some(next);
        }
        Ok(self.buffer.as_mut().expect("buffer initialized"))
    }
}

impl Write for Producer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let b = self.buffer()?;
        let n = bytes.len().min(BLOCK - b.used);
        let start = b.start + b.used;
        b.storage[start..start + n].copy_from_slice(&bytes[..n]);
        b.used += n;
        if b.used == BLOCK {
            self.flush()?;
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(block) = self.buffer.take().filter(|b| b.used != 0) {
            self.digest.update(block.bytes());
            self.submit.send(block).map_err(io::Error::other)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[test]
fn streamed_encoding_preserves_bytes_and_propagates_disk_failure() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("disk writer fault"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let text = Text::from("a猫\n".repeat(2 * BLOCK / 5 + 17).as_str());
    let codec = encoding_rs::UTF_8;
    let format = Format::default();
    let mut expected = Vec::new();
    write_encoded(&mut expected, &text, &format, codec).unwrap();
    let mut actual = Vec::new();
    let identity = encode(&mut actual, &text, &format, codec).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(identity.hash, *blake3::hash(&expected).as_bytes());
    assert_eq!(identity.bytes, expected.len() as u64);
    assert!(
        encode(&mut Broken, &text, &format, codec)
            .unwrap_err()
            .contains("disk writer fault")
    );
}
