//! Shared review byte samples and an independent strict-decoder oracle.
use vel_fileio as io;
use vel_text::Text;
pub struct Sample {
    pub name: &'static str,
    pub bytes: Vec<u8>,
    pub text: Option<&'static str>,
    pub codec: &'static str,
    pub bom: bool,
}
pub fn sample(name: &'static str, text: &'static str, codec: &'static str, bom: bool) -> Sample {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("encode");
    let mut format = io::encoding_format(codec, io::LineEnding::Lf).unwrap();
    format.bom = bom;
    io::save(&path, &Text::from(text), &format, false).unwrap();
    Sample {
        name,
        bytes: std::fs::read(path).unwrap(),
        text: Some(text),
        codec,
        bom,
    }
}
pub fn samples() -> Vec<Sample> {
    let mut samples = vec![
        sample("ASCII", "hello!\n", "UTF-8", false),
        sample("UTF-8", "naïve café ✓!", "UTF-8", false),
        sample("UTF-8 valid in Shift JIS", "naïve café", "UTF-8", false),
        sample("UTF-8 heuristic trap", "بܐبܐ", "UTF-8", false),
        sample("Shift JIS", "日本語かな\n", "Japanese (Shift JIS)", false),
        sample("GBK", "简体中文\n", "Chinese Simplified (GBK)", false),
        sample(
            "GBK heuristic trap",
            "亍堋佟荨",
            "Chinese Simplified (GBK)",
            false,
        ),
        sample("1252", "café €\n", "Western (Windows 1252)", false),
        sample(
            "1252 heuristic trap",
            "ØaÜbØcÜd",
            "Western (Windows 1252)",
            false,
        ),
        sample("UTF-8 BOM", "hello café\n", "UTF-8", true),
    ];
    for (name, codec, bom) in [
        ("UTF-16LE", "UTF-16 LE", false),
        ("UTF-16BE", "UTF-16 BE", false),
        ("UTF-16LE BOM", "UTF-16 LE", true),
        ("UTF-16BE BOM", "UTF-16 BE", true),
    ] {
        samples.push(sample(name, "hello 中文 😀\n", codec, bom));
    }
    samples.push(sample(
        "literal BOM after source BOM",
        "\u{feff}hello café\n",
        "UTF-8",
        true,
    ));
    let late_nul: &'static str = Box::leak(format!("{}\0é\n", "a".repeat(8200)).into_boxed_str());
    samples.extend([
        sample("UTF-8 NUL after prefix", late_nul, "UTF-8", false),
        sample(
            "1252 NUL after prefix",
            late_nul,
            "Western (Windows 1252)",
            false,
        ),
        Sample {
            name: "UTF-8 BOM NUL",
            bytes: b"\xef\xbb\xbfhello\0".to_vec(),
            text: None,
            codec: "UTF-8",
            bom: true,
        },
        Sample {
            name: "short ambiguous",
            bytes: vec![0, b'h'],
            text: None,
            codec: "UTF-16BE",
            bom: false,
        },
        Sample {
            name: "NUL binary",
            bytes: b"hello\0\x01\x02".to_vec(),
            text: None,
            codec: "UTF-8",
            bom: false,
        },
        Sample {
            name: "PNG",
            bytes: b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0".to_vec(),
            text: None,
            codec: "UTF-8",
            bom: false,
        },
    ]);
    samples
}
fn strict(codec: &str, bytes: &[u8]) -> Option<String> {
    io::encoding(codec)
        .unwrap()
        .decode_without_bom_handling_and_without_replacement(bytes)
        .map(|text| text.replace("\r\n", "\n").replace('\r', "\n"))
}
// Independent oracle: known source text/BOM, or the explicitly requested strict
// interpretation. Ambiguous bytes cannot identify their original legacy codec:
// a valid explicit reinterpretation wins, as required by the command contract.
pub fn expected(
    sample: &Sample,
    preferred: Option<&str>,
    fallback: &str,
) -> Option<(String, &'static str, bool)> {
    if matches!(sample.name, "PNG" | "UTF-8 BOM NUL") {
        return None;
    }
    let actual = io::encoding(sample.codec).unwrap();
    if sample.bom {
        return Some((
            sample.text.unwrap().into(),
            actual.name(),
            preferred.is_some_and(|p| io::encoding(p).unwrap() == actual),
        ));
    }
    let nuls = sample.bytes[..sample.bytes.len().min(8192)].contains(&0);
    if let Some(preferred) = preferred {
        let codec = io::encoding(preferred).unwrap();
        if (!nuls || codec.name() == "UTF-16LE" || codec.name() == "UTF-16BE")
            && let Some(text) = strict(codec.name(), &sample.bytes)
        {
            return Some((text, codec.name(), true));
        }
    }
    if nuls {
        if sample.name == "UTF-16LE" || sample.name == "UTF-16BE" {
            return Some((sample.text.unwrap().into(), actual.name(), false));
        }
        return None;
    }
    if let Some(text) = strict("UTF-8", &sample.bytes) {
        return Some((text, "UTF-8", false));
    }
    let codec = io::encoding(fallback).unwrap();
    let text = strict(codec.name(), &sample.bytes).unwrap_or_else(|| {
        codec
            .decode_without_bom_handling(&sample.bytes)
            .0
            .replace("\r\n", "\n")
            .replace('\r', "\n")
    });
    Some((text, codec.name(), false))
}
