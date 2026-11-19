//! Bounded staged edits resolved only against an immutable document snapshot.
use crate::tools::MAX_CONTEXT_BYTES;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

pub const MAX_EDITS: usize = 128;
const MAX_STAGED_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EditBatch {
    pub edits: Vec<Edit>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Edit {
    #[serde(default)]
    pub original: String,
    pub replacement: String,
    /// Exact text immediately before/after the original, used to disambiguate.
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    #[serde(default)]
    pub position: Position,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Position {
    #[default]
    Match,
    Start,
    End,
}

#[derive(Clone)]
struct ResolvedEdit {
    range: Range<usize>,
    replacement: String,
}

pub struct StagedEdits {
    snapshot: String,
    edits: Vec<ResolvedEdit>,
    full: Option<String>,
    chunked: bool,
    bytes: usize,
    finalized: bool,
}

impl StagedEdits {
    pub fn new(snapshot: String) -> Result<Self> {
        if snapshot.len() > MAX_CONTEXT_BYTES {
            bail!("Open document exceeds the 256 KiB Agent mode edit limit");
        }
        Ok(Self {
            snapshot,
            edits: Vec::new(),
            full: None,
            chunked: false,
            bytes: 0,
            finalized: false,
        })
    }

    pub fn pending(&self) -> bool {
        !self.edits.is_empty() || self.full.is_some()
    }

    pub fn ready(&self) -> Result<()> {
        if self.pending() && !self.finalized {
            bail!("Agent edits were not finalized; no edits applied");
        }
        Ok(())
    }

    fn mutable(&self) -> Result<()> {
        if self.finalized {
            bail!("Agent edit set is already finalized; no edits applied");
        }
        Ok(())
    }

    /// All-or-nothing validation of a batch; failed batches leave staging unchanged.
    pub fn stage(&mut self, batch: EditBatch) -> Result<()> {
        self.mutable()?;
        if self.full.is_some() {
            bail!("Cannot mix complete replacements and targeted edits; no edits applied");
        }
        if batch.edits.is_empty() || self.edits.len() + batch.edits.len() > MAX_EDITS {
            bail!("Agent edits require 1–128 changes per turn; no edits applied");
        }
        let bytes = batch.edits.iter().try_fold(self.bytes, |total, edit| {
            total
                .checked_add(edit.original.len())
                .and_then(|n| n.checked_add(edit.replacement.len()))
                .and_then(|n| n.checked_add(edit.prefix.len()))
                .and_then(|n| n.checked_add(edit.suffix.len()))
        });
        let Some(bytes) = bytes.filter(|n| *n <= MAX_STAGED_BYTES) else {
            bail!("Staged agent edits exceed 1 MiB; no edits applied");
        };
        let mut candidate = self.edits.clone();
        for edit in batch.edits {
            if edit.replacement.contains('\0') {
                bail!("Agent edits cannot contain NUL characters; no edits applied");
            }
            let range = self.resolve(&edit)?;
            candidate.push(ResolvedEdit {
                range,
                replacement: edit.replacement,
            });
        }
        candidate.sort_by_key(|edit| (edit.range.start, edit.range.end));
        for pair in candidate.windows(2) {
            let a = &pair[0].range;
            let b = &pair[1].range;
            // Adjacent replacements are allowed; insertions at their edges are ambiguous.
            if a.end > b.start || (a.end == b.start && (a.is_empty() || b.is_empty())) {
                bail!("Agent edit ranges overlap or share an insertion anchor; no edits applied");
            }
        }
        self.render(&candidate)?;
        self.edits = candidate;
        self.bytes = bytes;
        Ok(())
    }

    /// Compatibility fallback for small complete rewrites; still requires finalization.
    pub fn stage_full(&mut self, replacement: String) -> Result<()> {
        self.mutable()?;
        if self.pending() {
            bail!("Cannot mix complete replacements and targeted edits; no edits applied");
        }
        if replacement.len() > MAX_CONTEXT_BYTES || replacement.contains('\0') {
            bail!("Invalid complete replacement (256 KiB limit, no NUL); no edits applied");
        }
        self.full = Some(replacement);
        Ok(())
    }

    pub fn finalize(&mut self) -> Result<String> {
        self.mutable()?;
        if !self.pending() {
            bail!("No staged agent edits to finalize; no edits applied");
        }
        let result = if let Some(full) = &self.full {
            full.clone()
        } else {
            self.render(&self.edits)?
        };
        self.finalized = true;
        Ok(result)
    }

    /// Ordered complete-document chunks for new drafts/rewrites that cannot fit
    /// in one generation. Never append to the live document or original snapshot.
    pub fn stage_chunk(&mut self, text: &str) -> Result<()> {
        self.mutable()?;
        if !self.edits.is_empty() || (self.full.is_some() && !self.chunked) {
            bail!("Cannot mix document chunks with other edit formats; no edits applied");
        }
        let size = self
            .full
            .as_ref()
            .map_or(0, String::len)
            .saturating_add(text.len());
        if text.is_empty() || text.contains('\0') || size > MAX_CONTEXT_BYTES {
            bail!(
                "Invalid document chunk (nonempty text, 256 KiB total, no NUL); no edits applied"
            );
        }
        self.full.get_or_insert_with(String::new).push_str(text);
        self.chunked = true;
        Ok(())
    }

    fn resolve(&self, edit: &Edit) -> Result<Range<usize>> {
        let text = self.snapshot.as_str();
        let range = if edit.position != Position::Match {
            if !edit.original.is_empty() || !edit.prefix.is_empty() || !edit.suffix.is_empty() {
                bail!("Start/end insertions require empty original and context; no edits applied");
            }
            let offset = if edit.position == Position::Start {
                0
            } else {
                text.len()
            };
            offset..offset
        } else {
            if edit.original.is_empty() && edit.prefix.is_empty() && edit.suffix.is_empty() {
                bail!("Insertions require an exact anchor or start/end position; no edits applied");
            }
            // Include overlapping occurrences, unlike str::match_indices.
            let mut matches = text
                .char_indices()
                .map(|(i, _)| i)
                .chain([text.len()])
                .filter(|&start| {
                    text[start..].starts_with(&edit.original)
                        && text[..start].ends_with(&edit.prefix)
                        && text[start + edit.original.len()..].starts_with(&edit.suffix)
                });
            let Some(start) = matches.next() else {
                bail!("Agent original text or anchor was not found; no edits applied");
            };
            if matches.next().is_some() {
                bail!(
                    "Agent original text or anchor is ambiguous; add exact prefix/suffix context; no edits applied"
                );
            }
            start..start + edit.original.len()
        };
        let boundary =
            |pos| pos == text.len() || text.grapheme_indices(true).any(|(i, _)| i == pos);
        if !boundary(range.start) || !boundary(range.end) {
            bail!("Agent edit splits a Unicode grapheme or CRLF boundary; no edits applied");
        }
        if edit.original == edit.replacement {
            bail!("Agent edit has no changes; no edits applied");
        }
        Ok(range)
    }

    fn render(&self, edits: &[ResolvedEdit]) -> Result<String> {
        let mut size = self.snapshot.len();
        for edit in edits {
            size = size
                .checked_sub(edit.range.len())
                .and_then(|n| n.checked_add(edit.replacement.len()))
                .ok_or_else(|| anyhow::anyhow!("Agent edit size overflow; no edits applied"))?;
        }
        if size > MAX_CONTEXT_BYTES {
            bail!("Resulting document exceeds 256 KiB; no edits applied");
        }
        let mut text = String::with_capacity(size);
        let mut offset = 0;
        for edit in edits {
            text.push_str(&self.snapshot[offset..edit.range.start]);
            text.push_str(&edit.replacement);
            offset = edit.range.end;
        }
        text.push_str(&self.snapshot[offset..]);
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn batch(value: serde_json::Value) -> EditBatch {
        serde_json::from_value(value).unwrap()
    }
    #[test]
    fn multiple_batches_use_original_snapshot_and_preserve_unicode_crlf() {
        let mut staged = StagedEdits::new("title\r\ncafé 👩‍💻\r\nlast".into()).unwrap();
        staged
            .stage(batch(
                serde_json::json!({"edits":[{"original":"title","replacement":"Title"}]}),
            ))
            .unwrap();
        staged.stage(batch(serde_json::json!({"edits":[{"original":"café","replacement":"茶"},{"original":"last","replacement":""}]}))).unwrap();
        assert!(staged.ready().is_err());
        assert_eq!(staged.finalize().unwrap(), "Title\r\n茶 👩‍💻\r\n");
        staged.ready().unwrap();
        assert!(staged.finalize().is_err());
        assert!(staged.stage_full("late".into()).is_err());
    }
    #[test]
    fn repeated_text_requires_context_and_overlapping_occurrences_are_ambiguous() {
        let mut staged = StagedEdits::new("a cat\nb cat\naaa".into()).unwrap();
        for original in ["cat", "aa", "missing"] {
            assert!(
                staged
                    .stage(batch(
                        serde_json::json!({"edits":[{"original":original,"replacement":"new"}]})
                    ))
                    .is_err()
            );
        }
        staged
            .stage(batch(
                serde_json::json!({"edits":[{"original":"cat","replacement":"dog","prefix":"b "}]}),
            ))
            .unwrap();
        assert_eq!(staged.finalize().unwrap(), "a cat\nb dog\naaa");
    }
    #[test]
    fn insertions_support_anchors_start_end_and_empty_documents() {
        for (snapshot, edit, expected) in [
            (
                "",
                serde_json::json!({"position":"start","replacement":"new"}),
                "new",
            ),
            (
                "body",
                serde_json::json!({"position":"end","replacement":"!"}),
                "body!",
            ),
            (
                "a b",
                serde_json::json!({"prefix":"a ","suffix":"b","replacement":"new "}),
                "a new b",
            ),
        ] {
            let mut staged = StagedEdits::new(snapshot.into()).unwrap();
            staged
                .stage(batch(serde_json::json!({"edits":[edit]})))
                .unwrap();
            assert_eq!(staged.finalize().unwrap(), expected);
        }
    }
    #[test]
    fn invalid_batches_are_atomic_and_cannot_mix_or_split_graphemes() {
        let mut staged = StagedEdits::new("cafe\u{301}\r\nbody".into()).unwrap();
        for edits in [
            serde_json::json!([]),
            serde_json::json!([{"replacement":"unanchored"}]),
            serde_json::json!([{"original":"e","replacement":"x"}]),
            serde_json::json!([{"original":"\r","replacement":""}]),
            serde_json::json!([{"original":"body","replacement":"\u{0000}"}]),
            serde_json::json!([{"original":"body","replacement":"new"},{"original":"missing","replacement":"bad"}]),
            serde_json::json!([{"original":"body","replacement":"new"},{"original":"bod","replacement":"bad"}]),
            serde_json::json!([{"original":"body","replacement":"new"},{"position":"end","replacement":"!"}]),
        ] {
            assert!(
                staged
                    .stage(batch(serde_json::json!({"edits":edits})))
                    .is_err()
            );
            assert!(!staged.pending());
        }
        staged.stage_full("full rewrite".into()).unwrap();
        assert!(
            staged
                .stage(batch(
                    serde_json::json!({"edits":[{"original":"body","replacement":"new"}]})
                ))
                .is_err()
        );
        assert_eq!(staged.finalize().unwrap(), "full rewrite");
    }
    #[test]
    fn overlap_across_batches_and_size_limits_are_rejected() {
        let mut staged = StagedEdits::new("body".into()).unwrap();
        staged
            .stage(batch(
                serde_json::json!({"edits":[{"original":"body","replacement":"new"}]}),
            ))
            .unwrap();
        assert!(
            staged
                .stage(batch(
                    serde_json::json!({"edits":[{"original":"body","replacement":"again"}]})
                ))
                .is_err()
        );
        assert_eq!(staged.finalize().unwrap(), "new");
        let mut staged = StagedEdits::new("body".into()).unwrap();
        assert!(
            staged
                .stage_full("x".repeat(MAX_CONTEXT_BYTES + 1))
                .is_err()
        );
        assert!(staged.stage(batch(serde_json::json!({"edits":[{"position":"start","replacement":"x".repeat(MAX_CONTEXT_BYTES)}]}))).is_err());
        assert!(StagedEdits::new("x".repeat(MAX_CONTEXT_BYTES + 1)).is_err());
    }

    #[test]
    fn complete_document_chunks_support_new_drafts_and_rewrites_without_partial_results() {
        for snapshot in ["", "old document"] {
            let mut staged = StagedEdits::new(snapshot.into()).unwrap();
            staged.stage_chunk("New\r\ncafe").unwrap();
            staged.stage_chunk("\u{301} 👩‍💻\r\n").unwrap();
            assert!(staged.ready().is_err());
            assert!(staged.stage_full("other".into()).is_err());
            assert!(staged.stage_chunk("").is_err());
            assert!(staged.stage_chunk(&"x".repeat(MAX_CONTEXT_BYTES)).is_err());
            assert_eq!(staged.finalize().unwrap(), "New\r\ncafe\u{301} 👩‍💻\r\n");
            assert!(staged.stage_chunk("late").is_err());
        }
    }

    #[test]
    fn batch_count_and_aggregate_arguments_are_bounded_before_mutation() {
        let mut staged = StagedEdits::new("one".into()).unwrap();
        let edit = Edit {
            original: "one".into(),
            replacement: "ONE".into(),
            ..Edit::default()
        };
        assert!(
            staged
                .stage(EditBatch {
                    edits: vec![edit; MAX_EDITS + 1]
                })
                .is_err()
        );
        assert!(
            staged
                .stage(EditBatch {
                    edits: vec![Edit {
                        prefix: "x".repeat(MAX_STAGED_BYTES + 1),
                        replacement: "new".into(),
                        ..Edit::default()
                    }]
                })
                .is_err()
        );
        assert!(!staged.pending());
        assert!(staged.finalize().is_err());
    }
}
