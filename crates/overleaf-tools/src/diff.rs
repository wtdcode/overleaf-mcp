use std::collections::BTreeSet;

use overleaf_types::HistoryDiffSegment;
use similar::{ChangeTag, DiffTag, TextDiff};

use crate::workspace::Workspace;

/// One insertion or deletion of the history diff, located by its byte span
/// in the text it belongs to: the newer text for insertions, the older text
/// for deletions.
struct ChangeSpan {
    inserted: bool,
    start: usize,
    end: usize,
    authors: Vec<String>,
    start_ts: Option<i64>,
    end_ts: Option<i64>,
}

/// A file's net change between two history versions. Overleaf reports it as
/// character-level segments; both texts are rebuilt from them so the change
/// can be shown as a line diff, and every hunk is attributed to the people
/// whose segments fall inside it.
pub struct VersionDiff {
    old: String,
    new: String,
    spans: Vec<ChangeSpan>,
}

impl VersionDiff {
    pub fn from_segments(segments: &[HistoryDiffSegment]) -> Self {
        let mut old = String::new();
        let mut new = String::new();
        let mut spans = Vec::new();
        for segment in segments {
            if let Some(text) = &segment.u {
                old.push_str(text);
                new.push_str(text);
                continue;
            }
            let (inserted, text, target) = match (&segment.i, &segment.d) {
                (Some(text), _) => (true, text, &mut new),
                (None, Some(text)) => (false, text, &mut old),
                (None, None) => continue,
            };
            let start = target.len();
            target.push_str(text);
            let meta = segment.meta.as_ref();
            spans.push(ChangeSpan {
                inserted,
                start,
                end: target.len(),
                authors: meta
                    .map(|m| m.users.iter().map(|u| u.display()).collect())
                    .unwrap_or_default(),
                start_ts: meta.and_then(|m| m.start_ts),
                end_ts: meta.and_then(|m| m.end_ts),
            });
        }
        VersionDiff { old, new, spans }
    }

    pub fn is_unchanged(&self) -> bool {
        self.old == self.new
    }

    /// Byte offset where each line starts, followed by the text length, so
    /// the line range `a..b` covers bytes `starts[a]..starts[b]`. Lines end
    /// after their `\n`, matching how the line diff splits the text.
    fn line_starts(text: &str) -> Vec<usize> {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
        if starts.last() != Some(&text.len()) {
            starts.push(text.len());
        }
        starts
    }

    /// Unified-style hunks with `context` unchanged lines around each change.
    /// Every line shows its number in the older and in the newer version; each
    /// hunk header names who made the change and when.
    pub fn render(&self, context: usize) -> String {
        let diff = TextDiff::from_lines(&self.old, &self.new);
        let old_starts = Self::line_starts(&self.old);
        let new_starts = Self::line_starts(&self.new);
        let mut out = String::new();
        for group in diff.grouped_ops(context) {
            let (Some(first), Some(last)) = (group.first(), group.last()) else {
                continue;
            };
            let mut authors = BTreeSet::new();
            let mut earliest: Option<i64> = None;
            let mut latest: Option<i64> = None;
            for op in group.iter().filter(|op| op.tag() != DiffTag::Equal) {
                let (o, n) = (op.old_range(), op.new_range());
                let old_window = old_starts[o.start]..old_starts[o.end];
                let new_window = new_starts[n.start]..new_starts[n.end];
                for span in &self.spans {
                    let window = if span.inserted {
                        &new_window
                    } else {
                        &old_window
                    };
                    if span.start < window.end && span.end > window.start {
                        authors.extend(span.authors.iter().cloned());
                        if let Some(ts) = span.start_ts {
                            earliest = Some(earliest.map_or(ts, |cur| cur.min(ts)));
                        }
                        if let Some(ts) = span.end_ts {
                            latest = Some(latest.map_or(ts, |cur| cur.max(ts)));
                        }
                    }
                }
            }
            let when = match (earliest, latest) {
                (None, None) => String::new(),
                (a, b) => {
                    let (a, b) = (Workspace::format_ts(a.or(b)), Workspace::format_ts(b.or(a)));
                    if a == b { a } else { format!("{a} – {b}") }
                }
            };
            let attribution = match (authors.is_empty(), when.is_empty()) {
                (true, true) => String::new(),
                (true, false) => format!(" {when}"),
                (false, true) => {
                    format!(" by {}", authors.into_iter().collect::<Vec<_>>().join(", "))
                }
                (false, false) => format!(
                    " by {}, {when}",
                    authors.into_iter().collect::<Vec<_>>().join(", ")
                ),
            };
            let old_range = first.old_range().start..last.old_range().end;
            let new_range = first.new_range().start..last.new_range().end;
            out.push_str(&format!(
                "@@ -{},{} +{},{} @@{attribution}\n",
                old_range.start + 1,
                old_range.len(),
                new_range.start + 1,
                new_range.len()
            ));
            let number =
                |index: Option<usize>| index.map(|i| (i + 1).to_string()).unwrap_or_default();
            for op in &group {
                for change in diff.iter_changes(op) {
                    let marker = match change.tag() {
                        ChangeTag::Delete => '-',
                        ChangeTag::Insert => '+',
                        ChangeTag::Equal => ' ',
                    };
                    out.push_str(&format!(
                        "{marker}{:>6}{:>6}  {}\n",
                        number(change.old_index()),
                        number(change.new_index()),
                        change.value().trim_end_matches('\n')
                    ));
                }
            }
        }
        out
    }
}
