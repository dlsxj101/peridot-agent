use std::ops::Range;

use crate::state::{TranscriptEntry, TranscriptKind};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BlockKind {
    ToolInvocation,
    DiffRun,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TranscriptBlock {
    pub header_index: usize,
    pub body_range: Range<usize>,
    pub kind: BlockKind,
    pub body_line_count: usize,
}

/// Scans the transcript and identifies collapsible blocks:
///
///  - **ToolInvocation**: a non-indented `ToolStart` followed by indented
///    detail lines (`ToolStart`, `Diff`, `ToolOk`, `ToolFail` whose text
///    starts with `"  "`).  The block body starts at the first detail line
///    and ends just before the next non-detail entry.
///
///  - **DiffRun**: a contiguous run of `Diff` entries that are *not* already
///    captured by a ToolInvocation block.
pub fn identify_transcript_blocks(transcript: &[TranscriptEntry]) -> Vec<TranscriptBlock> {
    let mut blocks = Vec::new();
    let mut in_tool_block = false;
    let mut tool_header = 0usize;
    let mut tool_body_start = 0usize;
    let mut tool_body_lines = 0usize;

    let flush_tool = |blocks: &mut Vec<TranscriptBlock>,
                      header: usize,
                      body_start: usize,
                      body_end: usize,
                      lines: usize| {
        if body_start < body_end && lines > 0 {
            blocks.push(TranscriptBlock {
                header_index: header,
                body_range: body_start..body_end,
                kind: BlockKind::ToolInvocation,
                body_line_count: lines,
            });
        }
    };

    for (i, entry) in transcript.iter().enumerate() {
        let is_indented = entry.text.starts_with("  ");
        let is_tool_detail = is_indented
            && matches!(
                entry.kind,
                TranscriptKind::ToolStart
                    | TranscriptKind::ToolOk
                    | TranscriptKind::ToolFail
                    | TranscriptKind::Diff
            );

        if in_tool_block {
            if is_tool_detail {
                tool_body_lines += entry.text.lines().count().max(1);
                continue;
            }
            flush_tool(
                &mut blocks,
                tool_header,
                tool_body_start,
                i,
                tool_body_lines,
            );
            in_tool_block = false;
        }

        if entry.kind == TranscriptKind::ToolStart && !is_indented {
            in_tool_block = true;
            tool_header = i;
            tool_body_start = i + 1;
            tool_body_lines = 0;
            continue;
        }
    }
    if in_tool_block {
        let end = transcript.len();
        flush_tool(
            &mut blocks,
            tool_header,
            tool_body_start,
            end,
            tool_body_lines,
        );
    }

    // Second pass: collect standalone Diff runs not already inside a tool block.
    let in_tool: std::collections::HashSet<usize> =
        blocks.iter().flat_map(|b| b.body_range.clone()).collect();

    let mut diff_start: Option<usize> = None;
    let mut diff_lines = 0usize;

    let flush_diff = |blocks: &mut Vec<TranscriptBlock>, start: usize, end: usize, lines: usize| {
        if end > start + 1 && lines > 0 {
            blocks.push(TranscriptBlock {
                header_index: start,
                body_range: (start + 1)..end,
                kind: BlockKind::DiffRun,
                body_line_count: lines,
            });
        }
    };

    for (i, entry) in transcript.iter().enumerate() {
        if in_tool.contains(&i) {
            if let Some(s) = diff_start.take() {
                flush_diff(&mut blocks, s, i, diff_lines);
                diff_lines = 0;
            }
            continue;
        }
        if entry.kind == TranscriptKind::Diff {
            if diff_start.is_none() {
                diff_start = Some(i);
                diff_lines = 0;
            }
            diff_lines += entry.text.lines().count().max(1);
        } else {
            if let Some(s) = diff_start.take() {
                flush_diff(&mut blocks, s, i, diff_lines);
                diff_lines = 0;
            }
        }
    }
    if let Some(s) = diff_start.take() {
        flush_diff(&mut blocks, s, transcript.len(), diff_lines);
    }

    blocks.sort_by_key(|b| b.header_index);
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::TranscriptEntry;

    fn entry(kind: TranscriptKind, text: &str) -> TranscriptEntry {
        TranscriptEntry::new(kind, text)
    }

    #[test]
    fn identifies_tool_invocation_block() {
        let transcript = vec![
            entry(TranscriptKind::ToolStart, "file_patch running"),
            entry(TranscriptKind::Diff, "  - old line"),
            entry(TranscriptKind::Diff, "  + new line"),
            entry(TranscriptKind::ToolOk, "  file_patch done"),
            entry(TranscriptKind::ToolOk, "file_patch ok"),
        ];
        let blocks = identify_transcript_blocks(&transcript);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::ToolInvocation);
        assert_eq!(blocks[0].header_index, 0);
        assert_eq!(blocks[0].body_range, 1..4);
        assert_eq!(blocks[0].body_line_count, 3);
    }

    #[test]
    fn identifies_standalone_diff_run() {
        let transcript = vec![
            entry(TranscriptKind::Assistant, "Here is the diff:"),
            entry(TranscriptKind::Diff, "- old"),
            entry(TranscriptKind::Diff, "+ new"),
            entry(TranscriptKind::Diff, "+ another"),
            entry(TranscriptKind::Assistant, "Done."),
        ];
        let blocks = identify_transcript_blocks(&transcript);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::DiffRun);
        assert_eq!(blocks[0].header_index, 1);
        assert_eq!(blocks[0].body_range, 2..4);
    }

    #[test]
    fn no_blocks_for_short_content() {
        let transcript = vec![
            entry(TranscriptKind::User, "hello"),
            entry(TranscriptKind::Assistant, "world"),
        ];
        let blocks = identify_transcript_blocks(&transcript);
        assert!(blocks.is_empty());
    }
}
