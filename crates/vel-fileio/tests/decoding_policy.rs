//! Cross product of byte evidence, persisted choice state and both streaming executors.
use encoding_rs::{UTF_8, UTF_16BE, UTF_16LE};
use vel_fileio::{self as io, DecodeOptions, PrefixFacts};

/// Hang guard: stops a wait that would otherwise never end. It is not a latency
/// bound; the test checks the condition it waits for.
const HANG_GUARD: std::time::Duration = std::time::Duration::from_secs(120);

#[path = "support/decoding.rs"]
mod model;
use model::{expected, samples};

#[test]
fn decode_policy_exhaustive_inputs_and_streaming_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("policy-table.txt");
    let samples = samples();
    let mut count = 0;
    for sample in &samples {
        // The three configured fallbacks also cover every determinable legacy source.
        for fallback in [
            "Western (Windows 1252)",
            "Japanese (Shift JIS)",
            "Chinese Simplified (GBK)",
        ] {
            for (preferred, effective) in [
                (None, "UTF-8"),
                (Some("Japanese (Shift JIS)"), "Shift_JIS"),
                (Some("UTF-8"), "UTF-8"),
                (Some("Western (Windows 1252)"), "windows-1252"),
                (Some("UTF-16 LE"), "UTF-16LE"),
                (Some("Chinese Simplified (GBK)"), "GBK"),
                (Some("Japanese (Shift JIS)"), "UTF-8"), // after fallback
                (Some("UTF-8"), "windows-1252"),         // after fallback in the other direction
            ] {
                for saved in [false, true] {
                    let options = DecodeOptions {
                        encoding: preferred.map(str::to_owned),
                        effective_encoding: Some(effective.into()),
                        saved_in_effective_encoding: saved,
                        fallback_encoding: fallback.into(),
                        ..Default::default()
                    };
                    let accepted = preferred;
                    let expected = expected(sample, accepted, fallback);
                    let facts = PrefixFacts::inspect(&sample.bytes);
                    let decision = io::decide_decoding(&facts, &options).unwrap();
                    assert!(!decision.unchanged);
                    if let Some((_, codec, chosen)) = &expected {
                        assert!(
                            decision
                                .attempts
                                .iter()
                                .any(|a| a.encoding.name() == *codec && a.user_choice == *chosen),
                            "{}: {options:?}",
                            sample.name
                        );
                    }
                    std::fs::write(&path, &sample.bytes).unwrap();
                    for background in [false, true] {
                        let result = if background {
                            let task =
                                io::load_background_with_options(path.clone(), options.clone())
                                    .unwrap();
                            task.result.recv_timeout(HANG_GUARD).unwrap()
                        } else {
                            io::load_with_options(&path, &options, |_| {})
                        };
                        let context =
                            format!("{}: {options:?}, background={background}", sample.name);
                        match &expected {
                            Some((text, codec, chosen)) => {
                                let loaded = result.unwrap_or_else(|e| panic!("{context}: {e}"));
                                assert_eq!(&loaded.text.to_string(), text, "{context}");
                                assert_eq!(loaded.format.encoding, *codec, "{context}");
                                assert_eq!(loaded.encoding_chosen, *chosen, "{context}");
                                assert_eq!(
                                    loaded.disk_content.unwrap().bytes,
                                    sample.bytes.len() as u64
                                );
                                if !chosen && accepted.is_some() {
                                    assert!(
                                        loaded
                                            .warning
                                            .as_ref()
                                            .is_some_and(|s| s.contains("policy-table.txt")),
                                        "{context}"
                                    );
                                }
                                if fallback == sample.codec
                                    && accepted.is_none()
                                    && !facts.valid_utf8
                                {
                                    assert_eq!(Some(text.as_str()), sample.text);
                                }
                            }
                            None => {
                                assert!(result.is_err(), "{context}: {result:?}");
                            }
                        }
                        count += 1;
                    }
                    // Identity is a separate fact: exact byte equality makes a touch
                    // unchanged even when the preferred decoder would reinterpret it.
                    let identity = io::ContentIdentity {
                        bytes: sample.bytes.len() as u64,
                        hash: *blake3::hash(&sample.bytes).as_bytes(),
                    };
                    let mut complete = facts.clone();
                    complete.content = Some(identity);
                    let same = DecodeOptions {
                        saved_content: Some(identity),
                        ..options.clone()
                    };
                    let touch = io::decide_decoding(&complete, &same).unwrap();
                    assert!(touch.unchanged);
                    if !facts.binary_signature && sample.name != "UTF-8 BOM NUL" {
                        assert_eq!(touch.attempts[0].encoding, io::encoding(effective).unwrap());
                    }
                    let mut different = identity;
                    different.hash[0] ^= 1;
                    assert!(
                        !io::decide_decoding(
                            &complete,
                            &DecodeOptions {
                                saved_content: Some(different),
                                ..same
                            }
                        )
                        .unwrap()
                        .unchanged
                    );
                }
            }
        }
    }
    assert_eq!(count, 2016);
}

#[test]
fn decode_policy_fact_truth_table() {
    // Exhaust every supplied fact, including combinations a real prefix cannot
    // have, so priority is explicit rather than relying on sampling accidents.
    for bom in [
        None,
        Some((UTF_8, 3)),
        Some((UTF_16LE, 2)),
        Some((UTF_16BE, 2)),
    ] {
        for nuls in [false, true] {
            for valid_utf8 in [false, true] {
                for utf16 in [None, Some(UTF_16LE), Some(UTF_16BE)] {
                    for binary_signature in [false, true] {
                        for preferred in [
                            None,
                            Some("UTF-8"),
                            Some("Shift_JIS"),
                            Some("windows-1252"),
                            Some("UTF-16LE"),
                        ] {
                            for saved in [false, true] {
                                let facts = PrefixFacts {
                                    bom,
                                    nuls,
                                    valid_utf8,
                                    utf16,
                                    binary_signature,
                                    content: None,
                                    legacy_text_matches: false,
                                    revision_stamp_matches: false,
                                };
                                let state = DecodeOptions {
                                    encoding: preferred.map(str::to_owned),
                                    effective_encoding: Some("UTF-8".into()),
                                    saved_in_effective_encoding: saved,
                                    ..Default::default()
                                };
                                let decision = io::decide_decoding(&facts, &state).unwrap();
                                let verification = io::decide_decoding(
                                    &facts,
                                    &DecodeOptions {
                                        saved_content: Some(io::ContentIdentity {
                                            bytes: 123,
                                            hash: [1; 32],
                                        }),
                                        ..state.clone()
                                    },
                                )
                                .unwrap();
                                if nuls || bom.is_some() || binary_signature {
                                    assert!(
                                        verification
                                            .attempts
                                            .iter()
                                            .all(|attempt| !attempt.verify_legacy_revision)
                                    );
                                }

                                if bom.is_some_and(|(codec, _)| codec == UTF_8) && nuls {
                                    assert!(decision.attempts.is_empty());
                                    assert!(decision.binary.is_some());
                                } else if let Some((codec, skip)) = bom {
                                    assert!(!decision.attempts.is_empty());
                                    assert_eq!(decision.attempts[0].encoding, codec);
                                    assert_eq!(decision.attempts[0].skip, skip);
                                } else if binary_signature {
                                    assert!(decision.attempts.is_empty());
                                    assert!(decision.binary.is_some());
                                } else if !nuls && let Some(preferred) = preferred {
                                    assert_eq!(
                                        decision.attempts[0].encoding,
                                        io::encoding(preferred).unwrap()
                                    );
                                } else if preferred.is_none() && !nuls {
                                    assert_eq!(decision.attempts[0].encoding, UTF_8);
                                } else if preferred.is_none()
                                    && let Some(codec) = utf16
                                {
                                    assert_eq!(decision.attempts[0].encoding, codec);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
