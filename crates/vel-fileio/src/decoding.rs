//! The pure document decode decision and its bounded IO adapter.
use crate::{Codec, DecodeOptions, encoding};
use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252};
use std::{io::Read, path::Path};

/// Raw disk identity, before decoding, BOM removal or newline normalization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContentIdentity {
    /// Original byte length.
    pub bytes: u64,
    /// BLAKE3 digest computed as the bytes are read or written.
    pub hash: [u8; 32],
}
/// Bytes of a document's beginning that decide its decoding.
pub(crate) const PREFIX_BYTES: usize = 8192;
/// Bounded evidence, independent of the tab's choice and the load path.
#[derive(Clone, Debug)]
pub struct PrefixFacts {
    /// BOM codec and byte length.
    pub bom: Option<(&'static Encoding, usize)>,
    /// At least one NUL in the sample.
    pub nuls: bool,
    /// Valid UTF-8 (allowing a scalar split at the end of an 8 KiB prefix).
    pub valid_utf8: bool,
    /// Conservative UTF-16 evidence; only used for automatic detection with NULs.
    pub utf16: Option<&'static Encoding>,
    /// A recognized binary file signature, stronger than a text codec choice.
    pub binary_signature: bool,
    /// Available after streaming the complete raw source, for touch comparison.
    pub content: Option<ContentIdentity>,
    /// Validated saved-text/digest proof for a legacy revision with no raw identity.
    pub legacy_text_matches: bool,
    /// Metadata still matches the recorded revision; the final digest must confirm it.
    pub revision_stamp_matches: bool,
}
impl PrefixFacts {
    /// Inspects at most the first 8 KiB, without any document-sized work.
    pub fn inspect(bytes: &[u8]) -> Self {
        let bytes = &bytes[..bytes.len().min(PREFIX_BYTES)];
        let bom = if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
            Some((UTF_8, 3))
        } else if bytes.starts_with(&[0xff, 0xfe]) {
            Some((UTF_16LE, 2))
        } else if bytes.starts_with(&[0xfe, 0xff]) {
            Some((UTF_16BE, 2))
        } else {
            None
        };
        let valid_utf8 = match std::str::from_utf8(bytes) {
            Ok(_) => true,
            Err(e) => bytes.len() == PREFIX_BYTES && e.error_len().is_none(),
        };
        Self {
            bom,
            nuls: bytes.contains(&0),
            valid_utf8,
            utf16: vel_largefile::detect_utf16(bytes)
                .and_then(|name| Encoding::for_label(name.as_bytes())),
            binary_signature: binary_signature(bytes),
            content: None,
            legacy_text_matches: false,
            revision_stamp_matches: false,
        }
    }
    /// The byte evidence `decide_decoding` reads, without the revision proofs.
    fn same_bytes_evidence(&self, other: &Self) -> bool {
        self.bom == other.bom
            && self.nuls == other.nuls
            && self.valid_utf8 == other.valid_utf8
            && self.utf16 == other.utf16
            && self.binary_signature == other.binary_signature
    }
}
pub(super) fn binary_signature(bytes: &[u8]) -> bool {
    [
        b"\x89PNG\r\n\x1a\n".as_slice(),
        b"GIF87a",
        b"GIF89a",
        b"\xff\xd8\xff",
        b"PK\x03\x04",
        b"\x7fELF",
    ]
    .iter()
    .any(|signature| bytes.starts_with(signature))
}
/// A diagnostic template; the file name is added identically by every caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeNotice {
    /// The file's BOM contradicts the preferred codec.
    Bom { chosen: String, detected: String },
    /// Strict decoding of the preference failed, so this revision uses detection.
    Fallback { chosen: String, detected: String },
    /// A binary file is not decoded as text.
    Binary { ignored_choice: bool },
    /// The configured fallback also failed strict decoding.
    Replacement { encoding: String },
}
impl DecodeNotice {
    /// One non-modal wording for open, reload, restore and Reopen.
    pub fn message(&self, path: &Path) -> String {
        let detail = match self {
            Self::Bom { chosen, detected } => {
                format!("BOM selects {detected}; stored encoding {chosen} was overridden")
            }
            Self::Fallback { chosen, detected } => {
                format!("could not decode as {chosen}; using {detected} for this revision")
            }
            Self::Replacement { encoding } => format!(
                "invalid {encoding} sequences; using replacement characters for this revision"
            ),
            Self::Binary { ignored_choice } => format!(
                "binary file: use hexadecimal view{}",
                if *ignored_choice {
                    " (stored encoding ignored)"
                } else {
                    ""
                }
            ),
        };
        format!("{}: {detail}", path.display())
    }
}
/// A decoder attempt. Fallbacks never create or change the persistent tab choice.
#[derive(Clone, Debug)]
pub struct DecodeAttempt {
    /// Codec to try, without replacement characters.
    pub encoding: Codec,
    /// BOM bytes consumed before this decoder.
    pub skip: usize,
    /// Whether the source carries this codec's BOM.
    pub bom: bool,
    /// Whether this result implements the user's current preference.
    pub user_choice: bool,
    /// A legacy saved-text proof must match before this private attempt is accepted.
    /// This attempt never publishes prefix snapshots.
    pub verify_legacy_revision: bool,
    /// A matching metadata stamp permits previews, but acceptance requires the raw digest.
    pub verify_raw_revision: bool,
    /// Last-resort decoding replaces malformed sequences and carries a notice.
    pub replacement: bool,
    /// Diagnostic if this attempt succeeds.
    pub notice: Option<DecodeNotice>,
}
/// Complete ordered decision. Executors only try these decoders; they never invent fallbacks.
#[derive(Clone, Debug)]
pub struct DecodingDecision {
    /// Strict candidates, then a notice-bearing replacement attempt when permitted.
    pub attempts: Vec<DecodeAttempt>,
    /// Terminal binary result (possibly after a strict explicit UTF-16 attempt).
    pub binary: Option<DecodeNotice>,
    /// Exact raw bytes still match the last saved/loaded disk identity.
    pub unchanged: bool,
}
/// Decides all load paths from bounded evidence and persisted document state.
///
/// Order: BOM; explicit choice; UTF-8; UTF-16 only with NUL evidence; binary;
/// configured fallback. Strong binary signatures contradict even an explicit
/// choice. A plain save does not change the choice. Unchanged raw bytes use
/// their own effective codec, independently of that choice. NUL-bearing content
/// counts as binary or UTF-16 evidence and may override a non-UTF-16 choice.
pub fn decide_decoding(
    facts: &PrefixFacts,
    state: &DecodeOptions,
) -> Result<DecodingDecision, String> {
    let raw_unchanged = facts.content.is_some() && facts.content == state.saved_content;
    let unchanged = raw_unchanged
        || (state.saved_content.is_none()
            && (state.legacy_stamp_matches || facts.legacy_text_matches));
    let preferred = state.encoding.as_ref();
    let chosen = preferred
        .map(|name| encoding(name).map(|codec| (name, codec)))
        .transpose()?;
    let mut decision = DecodingDecision {
        attempts: Vec::new(),
        binary: None,
        unchanged,
    };
    if unchanged
        // Old binary views stored the default UTF-8 label, not a decoder.
        // NUL evidence cannot validate that legacy label from a stamp alone.
        && (raw_unchanged || facts.legacy_text_matches || !facts.nuls
            || state.effective_encoding.as_deref().is_some_and(|name| {
                encoding(name).is_ok_and(|codec| codec == UTF_16LE || codec == UTF_16BE)
            }))
        && !facts.binary_signature
        && !(facts.bom.is_some_and(|(c, _)| c == UTF_8) && facts.nuls)
        && let Some(name) = &state.effective_encoding
    {
        decision.attempts.push(DecodeAttempt {
            encoding: encoding(name)?,
            skip: facts
                .bom
                .filter(|(codec, _)| codec.name().eq_ignore_ascii_case(name))
                .map_or(0, |(_, skip)| skip),
            bom: facts
                .bom
                .is_some_and(|(codec, _)| codec.name().eq_ignore_ascii_case(name)),
            user_choice: chosen.is_some_and(|(_, c)| c == encoding(name).expect("validated codec")),
            verify_legacy_revision: false,
            verify_raw_revision: false,
            replacement: false,
            notice: None,
        });
        let mut replacement = decision.attempts[0].clone();
        replacement.replacement = true;
        replacement.notice = Some(DecodeNotice::Replacement {
            encoding: name.clone(),
        });
        decision.attempts.push(replacement);
        return Ok(decision);
    }
    if state.saved_content.is_some()
        && facts.content.is_none()
        && facts.revision_stamp_matches
        && !facts.binary_signature
        && state.effective_encoding.is_some()
    {
        let mut provisional = facts.clone();
        provisional.content = state.saved_content;
        let mut revision = decide_decoding(&provisional, state)?;
        if !revision.attempts.is_empty() {
            let mut changed = facts.clone();
            changed.revision_stamp_matches = false;
            let policy = decide_decoding(&changed, state)?;
            for attempt in &mut revision.attempts {
                attempt.verify_raw_revision = true;
            }
            revision.attempts.extend(policy.attempts);
            revision.binary = policy.binary;
            revision.unchanged = false; // The stamp is permission to preview, not proof.
            return Ok(revision);
        }
    }
    // UTF-8 cannot explain NUL evidence as byte order. UTF-16 BOMs can;
    // neither executor gets to invent a different text/binary rule later.
    if facts.bom.is_some_and(|(codec, _)| codec == UTF_8) && facts.nuls {
        decision.binary = Some(DecodeNotice::Binary {
            ignored_choice: chosen.is_some(),
        });
        return Ok(decision);
    }
    if let Some((codec, skip)) = facts.bom {
        let codec = Codec::Standard(codec);
        decision.attempts.push(DecodeAttempt {
            encoding: codec,
            skip,
            bom: true,
            user_choice: chosen.is_some_and(|(_, old)| old == codec),
            verify_legacy_revision: false,
            verify_raw_revision: false,
            replacement: false,
            notice: chosen
                .filter(|(_, old)| *old != codec)
                .map(|(name, _)| DecodeNotice::Bom {
                    chosen: name.clone(),
                    detected: codec.name().into(),
                }),
        });
        // Preserve the existing lossless legacy fallback for malformed BOM data.
        // It consumes the original bytes (including the invalid marker), names
        // the failed BOM codec, and never becomes a user encoding choice.
        let fallback = encoding(&state.fallback_encoding).unwrap_or(Codec::Standard(WINDOWS_1252));
        if fallback != codec {
            decision.attempts.push(DecodeAttempt {
                encoding: fallback,
                skip: 0,
                bom: false,
                user_choice: false,
                verify_legacy_revision: false,
                verify_raw_revision: false,
                replacement: false,
                notice: Some(DecodeNotice::Fallback {
                    chosen: codec.name().into(),
                    detected: fallback.name().into(),
                }),
            });
        }
        decision.attempts.push(DecodeAttempt {
            encoding: fallback,
            skip: if fallback == codec { skip } else { 0 },
            bom: fallback == codec,
            user_choice: false,
            verify_legacy_revision: false,
            verify_raw_revision: false,
            replacement: true,
            notice: Some(DecodeNotice::Replacement {
                encoding: fallback.name().into(),
            }),
        });
        return Ok(decision);
    }
    if facts.binary_signature {
        decision.binary = Some(DecodeNotice::Binary {
            ignored_choice: chosen.is_some(),
        });
        return Ok(decision);
    }
    let mut push = |codec: Codec, explicit: bool| {
        if !decision
            .attempts
            .iter()
            .any(|attempt| attempt.encoding == codec)
        {
            decision.attempts.push(DecodeAttempt {
                encoding: codec,
                skip: 0,
                bom: false,
                user_choice: explicit,
                verify_legacy_revision: false,
                verify_raw_revision: false,
                replacement: false,
                notice: if explicit {
                    None
                } else {
                    chosen.map(|(name, _)| DecodeNotice::Fallback {
                        chosen: name.clone(),
                        detected: codec.name().into(),
                    })
                },
            });
        }
    };
    if let Some((_, codec)) = chosen {
        // ASCII-compatible codecs preserve NULs. Those cannot be text here;
        // an explicitly selected UTF-16 decoder can resolve a short ambiguous sample.
        if !facts.nuls || codec == UTF_16LE || codec == UTF_16BE {
            push(codec, true);
        }
    }
    if facts.valid_utf8 && !facts.nuls {
        push(Codec::Standard(UTF_8), false);
    }
    if facts.nuls {
        if let Some(codec) = facts.utf16 {
            push(Codec::Standard(codec), false);
        } else {
            decision.binary = Some(DecodeNotice::Binary {
                ignored_choice: chosen.is_some(),
            });
        }
    } else {
        // The prefix can be legacy text or valid UTF-8 until a later chunk fails.
        // Try UTF-8 before the configured legacy decoder in either case.
        push(Codec::Standard(UTF_8), false);
        // Keep this function pure: reporting an invalid preference is the
        // settings loader's job, not the once-per-process diagnostic registry.
        let fallback = encoding(&state.fallback_encoding).unwrap_or(Codec::Standard(WINDOWS_1252));
        push(fallback, false);
    }
    if decision.binary.is_none() {
        let fallback = encoding(&state.fallback_encoding).unwrap_or(Codec::Standard(WINDOWS_1252));
        decision.attempts.push(DecodeAttempt {
            encoding: fallback,
            skip: 0,
            bom: false,
            user_choice: false,
            verify_legacy_revision: false,
            verify_raw_revision: false,
            replacement: true,
            notice: Some(DecodeNotice::Replacement {
                encoding: fallback.name().into(),
            }),
        });
    }
    // Old native histories have a text digest/saved snapshot, but no raw digest.
    // Validate that proof in private; a mismatch resumes the changed-byte policy.
    if state.saved_content.is_none()
        && (state.legacy_saved_text.is_some() || state.legacy_text_fingerprint.is_some())
        && let Some(name) = &state.effective_encoding
    {
        let codec = encoding(name)?;
        decision.attempts.insert(
            0,
            DecodeAttempt {
                encoding: codec,
                skip: 0,
                bom: false,
                user_choice: chosen.is_some_and(|(_, old)| old == codec),
                verify_legacy_revision: true,
                verify_raw_revision: false,
                replacement: false,
                notice: None,
            },
        );
    }
    Ok(decision)
}
/// Reads bounded evidence and applies the same pure policy used by the decoders.
pub fn decoding_for_path(
    path: &Path,
    options: &DecodeOptions,
) -> Result<(PrefixFacts, DecodingDecision), String> {
    let result = (|| {
        let mut prefix = Vec::with_capacity(PREFIX_BYTES);
        crate::regular::read(path)?
            .take(PREFIX_BYTES as u64)
            .read_to_end(&mut prefix)
            .map_err(|e| e.to_string())?;
        let mut facts = PrefixFacts::inspect(&prefix);
        facts.revision_stamp_matches = options.revision_stamp.is_some_and(|saved| {
            std::fs::metadata(path).ok().and_then(|m| {
                Some((
                    m.len(),
                    m.modified()
                        .ok()?
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()?
                        .as_nanos(),
                ))
            }) == Some(saved)
        });
        let decision = decide_decoding(&facts, options)?;
        Ok((facts, decision))
    })();
    result.map_err(|error| crate::load_error(path, error))
}

/// One streaming plan shared by foreground and background executors. The common
/// unchanged restore reads the source once, publishes prefixes immediately, and
/// verifies identity at EOF. Changed stamps run the normal policy without a pre-pass.
pub(crate) struct DecodePlan {
    pub facts: PrefixFacts,
    decision: DecodingDecision,
    index: usize,
    redecided: usize,
}
#[cfg(test)]
type Hook = Box<dyn FnOnce() + Send>;
/// Test hooks that run once, right after a plan for their path sampled its prefix.
#[cfg(test)]
pub(crate) static AFTER_DECISION: std::sync::Mutex<Vec<(std::path::PathBuf, Hook)>> =
    std::sync::Mutex::new(Vec::new());
impl DecodePlan {
    pub fn new(path: &Path, options: &DecodeOptions) -> Result<Self, String> {
        let (facts, decision) = decoding_for_path(path, options)?;
        #[cfg(test)]
        {
            let hook = {
                let mut hooks = AFTER_DECISION.lock().expect("decision hooks");
                let found = hooks.iter().position(|(hooked, _)| hooked == path);
                found.map(|i| hooks.remove(i).1)
            };
            if let Some(hook) = hook {
                hook();
            }
        }
        Ok(Self {
            facts,
            decision,
            index: 0,
            redecided: 0,
        })
    }
    /// The decision comes from a prefix sampled before the attempt opened the file.
    /// When the bytes the attempt read begin with different evidence, the file changed
    /// in between and the decoded text would mix one version's decision with the
    /// other's bytes (and record the new bytes' identity as if they had been decided).
    /// Decide again from what was read and restart. A file that keeps changing is
    /// eventually accepted as read; its changed stamp makes the caller read it again.
    pub fn read_matches_decision(
        &mut self,
        options: &DecodeOptions,
        read: &[u8],
    ) -> Result<bool, String> {
        let read = PrefixFacts::inspect(read);
        if self.redecided >= 4 || read.same_bytes_evidence(&self.facts) {
            return Ok(true);
        }
        self.redecided += 1;
        self.decision = decide_decoding(&read, options)?;
        self.facts = read;
        self.index = 0;
        Ok(false)
    }
    pub fn next(&mut self) -> Option<DecodeAttempt> {
        let attempt = self.decision.attempts.get(self.index)?.clone();
        self.index += 1;
        Some(attempt)
    }
    pub fn binary(&self) -> Option<&DecodeNotice> {
        self.decision.binary.as_ref()
    }
    pub fn accept(
        &mut self,
        options: &DecodeOptions,
        attempt: &DecodeAttempt,
        loaded: &crate::Loaded,
    ) -> Result<bool, String> {
        // A touch with a different stamp is discovered during the policy decode.
        // If the explicit choice differs from the revision codec, retry that codec
        // before accepting. No second read occurs when stamps and hashes match.
        if loaded.disk_content.is_some() && loaded.disk_content == options.saved_content {
            let mut verified = self.facts.clone();
            verified.content = loaded.disk_content;
            let revision = decide_decoding(&verified, options)?;
            if revision.attempts.first().is_some_and(|first| {
                first.encoding != attempt.encoding || first.skip != attempt.skip
            }) {
                self.facts = verified;
                self.decision = revision;
                self.index = 0;
                return Ok(false);
            }
        }
        if attempt.verify_raw_revision && loaded.disk_content != options.saved_content {
            self.facts.content = loaded.disk_content;
            self.decision = decide_decoding(&self.facts, options)?;
            self.index = 0;
            return Ok(false);
        }
        Ok(
            (!attempt.verify_raw_revision || loaded.disk_content == options.saved_content)
                && (!attempt.verify_legacy_revision || loaded.unchanged),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rewrite_between_decision_and_read_is_decoded_by_its_own_evidence() {
        // An external tool rewrote a UTF-16 file as ASCII after the loader sampled the
        // prefix. Both executors then decoded the new bytes with the old decision
        // (UTF-16, then the windows-1252 replacement attempt) and recorded the new bytes'
        // identity, so every later reload kept windows-1252 as "unchanged".
        let dir = tempfile::tempdir().unwrap();
        for background in [false, true] {
            let path = dir.path().join(format!("rewritten-{background}.txt"));
            let utf16: Vec<u8> = "base text\n"
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect();
            std::fs::write(&path, utf16).unwrap();
            let rewritten = path.clone();
            AFTER_DECISION.lock().unwrap().push((
                path.clone(),
                Box::new(move || std::fs::write(rewritten, b"odd!\n").unwrap()),
            ));
            let options = DecodeOptions {
                encoding: Some("UTF-16 LE".into()),
                fallback_encoding: "Western (Windows 1252)".into(),
                ..Default::default()
            };
            let loaded = if background {
                crate::load_background_with_options(path.clone(), options)
                    .unwrap()
                    .result
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap()
            } else {
                crate::load_with_options(&path, &options, |_| {})
            }
            .unwrap();
            assert!(AFTER_DECISION.lock().unwrap().is_empty(), "hook ran");
            assert_eq!(loaded.text.to_string(), "odd!\n", "background={background}");
            assert_eq!(loaded.format.encoding, "UTF-8", "background={background}");
            assert!(!loaded.encoding_chosen);
            assert_eq!(loaded.disk_content, crate::content_identity(&path).unwrap());
        }
    }
}
