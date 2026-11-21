//! Supported text codecs, including the two DOS pages not in WHATWG Encoding.
use encoding_rs::{CoderResult, Encoding};
use std::borrow::Cow;

/// A resolved text codec with a canonical name for session persistence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// A standard encoding_rs codec.
    Standard(&'static Encoding),
    /// IBM Hebrew DOS.
    Cp862,
    /// IBM Nordic DOS.
    Cp865,
}

impl PartialEq<&'static Encoding> for Codec {
    fn eq(&self, other: &&'static Encoding) -> bool {
        matches!(self, Self::Standard(codec) if codec == other)
    }
}

impl Codec {
    /// Canonical name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Standard(codec) => codec.name(),
            Self::Cp862 => "CP862",
            Self::Cp865 => "CP865",
        }
    }

    pub(crate) fn is_single_byte(self) -> bool {
        match self {
            Self::Standard(codec) => codec.is_single_byte(),
            _ => true,
        }
    }

    fn scalar(self, byte: u8) -> char {
        let value = match (self, byte) {
            (_, 0..=127) => u32::from(byte),
            (Self::Cp862, 0x80..=0x9a) => 0x05d0 + u32::from(byte - 0x80),
            (Self::Cp862, 0x9b) => 0x00a2,
            (Self::Cp862, 0x9d) => 0x00a5,
            (Self::Cp862, 0xaf) => 0x00bb,
            _ => u32::from(CP865[usize::from(byte - 128)]),
        };
        char::from_u32(value).expect("valid code-page scalar")
    }

    /// Decodes bytes without recognizing a leading BOM.
    pub fn decode_without_bom_handling(self, bytes: &[u8]) -> (Cow<'_, str>, bool) {
        match self {
            Self::Standard(codec) => codec.decode_without_bom_handling(bytes),
            _ => (
                bytes
                    .iter()
                    .map(|&byte| self.scalar(byte))
                    .collect::<String>()
                    .into(),
                false,
            ),
        }
    }

    /// Strict version of BOM-free decoding.
    pub fn decode_without_bom_handling_and_without_replacement(
        self,
        bytes: &[u8],
    ) -> Option<Cow<'_, str>> {
        match self {
            Self::Standard(codec) => {
                codec.decode_without_bom_handling_and_without_replacement(bytes)
            }
            _ => Some(self.decode_without_bom_handling(bytes).0),
        }
    }

    /// Returns the bytes, output codec and whether a scalar was unrepresentable.
    pub fn encode(self, text: &str) -> (Cow<'_, [u8]>, Self, bool) {
        match self {
            Self::Standard(codec) => {
                let (bytes, codec, errors) = codec.encode(text);
                (bytes, Self::Standard(codec), errors)
            }
            _ => {
                let mut bytes = Vec::with_capacity(text.len());
                for ch in text.chars() {
                    if ch.is_ascii() {
                        bytes.push(ch as u8);
                    } else if let Some(byte) = (128..=255).find(|&byte| self.scalar(byte) == ch) {
                        bytes.push(byte);
                    } else {
                        return (Cow::Owned(bytes), self, true);
                    }
                }
                (Cow::Owned(bytes), self, false)
            }
        }
    }

    pub(crate) fn decoder(self) -> Decoder {
        match self {
            Self::Standard(codec) => Decoder::Standard(codec.new_decoder_without_bom_handling()),
            _ => Decoder::Dos(self),
        }
    }
}

pub(crate) enum Decoder {
    Standard(encoding_rs::Decoder),
    Dos(Codec),
}

impl Decoder {
    pub(crate) fn decode_to_utf8(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        eof: bool,
    ) -> (CoderResult, usize, usize, bool) {
        match self {
            Self::Standard(decoder) => decoder.decode_to_utf8(input, output, eof),
            Self::Dos(codec) => {
                let mut written = 0;
                for (read, &byte) in input.iter().enumerate() {
                    let scalar = codec.scalar(byte);
                    let count = scalar.len_utf8();
                    if written + count > output.len() {
                        return (CoderResult::OutputFull, read, written, false);
                    }
                    scalar.encode_utf8(&mut output[written..written + count]);
                    written += count;
                }
                (CoderResult::InputEmpty, input.len(), written, false)
            }
        }
    }
}

// Upper half of IBM865; IBM862 has the same box-drawing and math characters
// but replaces the Hebrew range and three DOS currency/punctuation positions.
const CP865: [u16; 128] = [
    0x00c7, 0x00fc, 0x00e9, 0x00e2, 0x00e4, 0x00e0, 0x00e5, 0x00e7, 0x00ea, 0x00eb, 0x00e8, 0x00ef,
    0x00ee, 0x00ec, 0x00c4, 0x00c5, 0x00c9, 0x00e6, 0x00c6, 0x00f4, 0x00f6, 0x00f2, 0x00fb, 0x00f9,
    0x00ff, 0x00d6, 0x00dc, 0x00f8, 0x00a3, 0x00d8, 0x20a7, 0x0192, 0x00e1, 0x00ed, 0x00f3, 0x00fa,
    0x00f1, 0x00d1, 0x00aa, 0x00ba, 0x00bf, 0x2310, 0x00ac, 0x00bd, 0x00bc, 0x00a1, 0x00ab, 0x00a4,
    0x2591, 0x2592, 0x2593, 0x2502, 0x2524, 0x2561, 0x2562, 0x2556, 0x2555, 0x2563, 0x2551, 0x2557,
    0x255d, 0x255c, 0x255b, 0x2510, 0x2514, 0x2534, 0x252c, 0x251c, 0x2500, 0x253c, 0x255e, 0x255f,
    0x255a, 0x2554, 0x2569, 0x2566, 0x2560, 0x2550, 0x256c, 0x2567, 0x2568, 0x2564, 0x2565, 0x2559,
    0x2558, 0x2552, 0x2553, 0x256b, 0x256a, 0x2518, 0x250c, 0x2588, 0x2584, 0x258c, 0x2590, 0x2580,
    0x03b1, 0x00df, 0x0393, 0x03c0, 0x03a3, 0x03c3, 0x00b5, 0x03c4, 0x03a6, 0x0398, 0x03a9, 0x03b4,
    0x221e, 0x03c6, 0x03b5, 0x2229, 0x2261, 0x00b1, 0x2265, 0x2264, 0x2320, 0x2321, 0x00f7, 0x2248,
    0x00b0, 0x2219, 0x00b7, 0x221a, 0x207f, 0x00b2, 0x25a0, 0x00a0,
];
