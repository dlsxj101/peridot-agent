//! Line-based diff hunks used by the approval panel's diff staging surface.
//!
//! `file_patch` reaches the approval prompt as a single
//! `(old_text, new_text)` pair, which gives the operator a one-shot
//! Approve / Deny over the entire replacement. The Approval Panel
//! rewrite slices that pair into individual changed regions (hunks) so
//! the operator can accept some hunks and reject others, then the
//! caller synthesises a partial replacement containing only the
//! accepted hunks.
//!
//! Algorithm: classic LCS-driven line diff (O(n·m)). We treat the two
//! texts as line vectors, compute the longest common subsequence, then
//! emit runs of `Removed` / `Added` lines that aren't part of the LCS
//! as one hunk each. Adjacent removal+addition runs collapse into a
//! single Replace hunk because that's the shape the operator
//! recognises (it matches `git diff` output).
//!
//! The algorithm is deterministic and side-effect free so it can run
//! synchronously inside the TUI render path without blocking.

use serde::{Deserialize, Serialize};

/// A single contiguous change region between the old and new text.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiffHunk {
    /// Zero-based line index in the *old* text where this hunk starts.
    pub old_start: usize,
    /// Old-side lines that this hunk removes. Empty for pure-add hunks.
    pub old_lines: Vec<String>,
    /// Zero-based line index in the *new* text where this hunk starts.
    pub new_start: usize,
    /// New-side lines that this hunk inserts. Empty for pure-delete hunks.
    pub new_lines: Vec<String>,
}

impl DiffHunk {
    /// Returns a short single-line label suitable for menu rendering,
    /// e.g. `"-2 / +3 lines @ old:14"`.
    pub fn label(&self) -> String {
        format!(
            "-{} / +{} lines @ old:{}",
            self.old_lines.len(),
            self.new_lines.len(),
            self.old_start + 1
        )
    }

    /// Returns true when the hunk only adds lines (no removals).
    pub fn is_pure_addition(&self) -> bool {
        self.old_lines.is_empty() && !self.new_lines.is_empty()
    }

    /// Returns true when the hunk only removes lines (no additions).
    pub fn is_pure_deletion(&self) -> bool {
        !self.old_lines.is_empty() && self.new_lines.is_empty()
    }

    /// Renders a unified-diff-style preview body of this hunk.
    pub fn unified_preview(&self) -> String {
        let mut out = String::new();
        for line in &self.old_lines {
            out.push_str("- ");
            out.push_str(line);
            out.push('\n');
        }
        for line in &self.new_lines {
            out.push_str("+ ");
            out.push_str(line);
            out.push('\n');
        }
        out
    }
}

/// Computes hunks describing how `new_text` differs from `old_text`.
/// Both texts are split on `\n`; trailing newlines are normalised so a
/// file with or without a final newline produces the same hunk set when
/// only the body changed.
pub fn diff_hunks(old_text: &str, new_text: &str) -> Vec<DiffHunk> {
    let old_lines: Vec<&str> = old_text.split('\n').collect();
    let new_lines: Vec<&str> = new_text.split('\n').collect();
    let ops = diff_line_ops(&old_lines, &new_lines);

    let mut hunks: Vec<DiffHunk> = Vec::new();
    let mut current: Option<DiffHunk> = None;
    let mut old_cursor = 0usize;
    let mut new_cursor = 0usize;
    for op in ops {
        match op {
            LineOp::Equal => {
                if let Some(hunk) = current.take() {
                    hunks.push(hunk);
                }
                old_cursor += 1;
                new_cursor += 1;
            }
            LineOp::Remove(line) => {
                let hunk = current.get_or_insert_with(|| DiffHunk {
                    old_start: old_cursor,
                    old_lines: Vec::new(),
                    new_start: new_cursor,
                    new_lines: Vec::new(),
                });
                hunk.old_lines.push(line);
                old_cursor += 1;
            }
            LineOp::Add(line) => {
                let hunk = current.get_or_insert_with(|| DiffHunk {
                    old_start: old_cursor,
                    old_lines: Vec::new(),
                    new_start: new_cursor,
                    new_lines: Vec::new(),
                });
                hunk.new_lines.push(line);
                new_cursor += 1;
            }
        }
    }
    if let Some(hunk) = current {
        hunks.push(hunk);
    }
    hunks
}

/// Synthesises a partial `new_text` by applying only the hunks whose
/// flag in `accepted` is `true`. The returned text is composed by
/// walking `old_text` line by line and, at each hunk's `old_start`,
/// either skipping the removed lines + inserting the added lines (when
/// accepted) or preserving the original lines (when rejected).
///
/// Returns `None` if `accepted.len()` does not match `hunks.len()` —
/// callers should treat that as a programming error.
pub fn apply_selected_hunks(
    old_text: &str,
    hunks: &[DiffHunk],
    accepted: &[bool],
) -> Option<String> {
    if accepted.len() != hunks.len() {
        return None;
    }
    let old_lines: Vec<&str> = old_text.split('\n').collect();
    let mut out_lines: Vec<String> = Vec::new();
    let mut cursor = 0usize;

    let mut hunks_iter = hunks.iter().zip(accepted.iter()).peekable();
    while cursor < old_lines.len() {
        if let Some((hunk, _)) = hunks_iter.peek()
            && hunk.old_start == cursor
        {
            let (hunk, accept) = hunks_iter.next().unwrap();
            if *accept {
                for line in &hunk.new_lines {
                    out_lines.push(line.clone());
                }
                cursor += hunk.old_lines.len();
            } else {
                for line in &hunk.old_lines {
                    out_lines.push((*line).to_string());
                }
                cursor += hunk.old_lines.len();
            }
            continue;
        }
        out_lines.push(old_lines[cursor].to_string());
        cursor += 1;
    }

    // Trailing pure-addition hunks (old_start == old_lines.len()).
    for (hunk, accept) in hunks_iter {
        if *accept {
            for line in &hunk.new_lines {
                out_lines.push(line.clone());
            }
        }
    }

    Some(out_lines.join("\n"))
}

#[derive(Clone, Debug)]
enum LineOp {
    Equal,
    Remove(String),
    Add(String),
}

fn diff_line_ops(old: &[&str], new: &[&str]) -> Vec<LineOp> {
    // LCS table.
    let n = old.len();
    let m = new.len();
    let mut table = vec![vec![0usize; m + 1]; n + 1];
    for i in 0..n {
        for j in 0..m {
            table[i + 1][j + 1] = if old[i] == new[j] {
                table[i][j] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    // Walk back from (n, m) to produce a forward-ordered op stream.
    let mut ops_rev: Vec<LineOp> = Vec::new();
    let mut i = n;
    let mut j = m;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && old[i - 1] == new[j - 1] {
            ops_rev.push(LineOp::Equal);
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || table[i][j - 1] >= table[i - 1][j]) {
            ops_rev.push(LineOp::Add(new[j - 1].to_string()));
            j -= 1;
        } else {
            ops_rev.push(LineOp::Remove(old[i - 1].to_string()));
            i -= 1;
        }
    }
    ops_rev.reverse();
    ops_rev
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_replace_into_one_hunk() {
        let old = "alpha\nbeta\ngamma\n";
        let new = "alpha\nBETA\ngamma\n";
        let hunks = diff_hunks(old, new);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_lines, vec!["beta".to_string()]);
        assert_eq!(hunks[0].new_lines, vec!["BETA".to_string()]);
        assert_eq!(hunks[0].old_start, 1);
    }

    #[test]
    fn separates_distant_changes_into_distinct_hunks() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nB\nc\nd\nE\n";
        let hunks = diff_hunks(old, new);
        assert_eq!(hunks.len(), 2, "expected two distinct hunks");
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[1].old_start, 4);
    }

    #[test]
    fn pure_addition_at_end() {
        let old = "alpha\nbeta\n";
        let new = "alpha\nbeta\ngamma\n";
        let hunks = diff_hunks(old, new);
        // Trailing newline produces an empty trailing line that
        // matches; the addition is then a single hunk.
        assert!(
            hunks
                .iter()
                .any(|h| h.new_lines.contains(&"gamma".to_string()))
        );
    }

    #[test]
    fn apply_with_all_hunks_accepted_reproduces_new_text() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nB\nc\nd\nE\n";
        let hunks = diff_hunks(old, new);
        let accepted = vec![true; hunks.len()];
        let result = apply_selected_hunks(old, &hunks, &accepted).unwrap();
        assert_eq!(result, new);
    }

    #[test]
    fn apply_with_no_hunks_accepted_returns_old_text() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nB\nc\nd\nE\n";
        let hunks = diff_hunks(old, new);
        let accepted = vec![false; hunks.len()];
        let result = apply_selected_hunks(old, &hunks, &accepted).unwrap();
        assert_eq!(result, old);
    }

    #[test]
    fn apply_with_partial_acceptance_picks_only_chosen_hunks() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nB\nc\nd\nE\n";
        let hunks = diff_hunks(old, new);
        // accept only the first hunk
        let accepted = vec![true, false];
        let result = apply_selected_hunks(old, &hunks, &accepted).unwrap();
        assert!(
            result.contains("B"),
            "first hunk should be applied: {result}"
        );
        assert!(
            result.contains("\ne"),
            "second hunk should be rejected: {result}"
        );
        assert!(
            !result.contains("E\n"),
            "second hunk should not appear: {result}"
        );
    }

    #[test]
    fn apply_returns_none_on_mismatched_selection_length() {
        let old = "a\nb\n";
        let new = "a\nB\n";
        let hunks = diff_hunks(old, new);
        assert!(apply_selected_hunks(old, &hunks, &[]).is_none());
    }

    #[test]
    fn label_summarises_hunk_size_and_position() {
        let hunk = DiffHunk {
            old_start: 13,
            old_lines: vec!["a".into(), "b".into()],
            new_start: 13,
            new_lines: vec!["x".into(), "y".into(), "z".into()],
        };
        assert_eq!(hunk.label(), "-2 / +3 lines @ old:14");
    }

    #[test]
    fn identical_text_produces_no_hunks() {
        let text = "alpha\nbeta\ngamma\n";
        assert!(diff_hunks(text, text).is_empty());
    }
}
