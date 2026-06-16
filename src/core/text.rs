//! Pure text algorithms: paragraph detection and sentence boundaries.
//!
//! These operate on plain line slices so they stay UI-free and unit-testable.
//! The editor widget (iced `text_editor`) owns the actual text storage; the
//! app extracts lines, asks these functions for positions, and applies the
//! result back as cursor moves/selections.
//!
//! Word boundaries are *not* here: iced's `Motion::WordLeft`/`WordRight`
//! already implement them.

/// A position as (line, column). Columns are **byte** offsets within the
/// line, matching iced's `text_editor::Position` (cosmic-text cursor index).
pub type Pos = (usize, usize);

fn is_blank(line: &str) -> bool {
    line.chars().all(char::is_whitespace)
}

/// Inclusive `(start_line, end_line)` ranges of non-blank runs.
pub fn paragraphs(lines: &[String]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, line) in lines.iter().enumerate() {
        match (is_blank(line), start) {
            (false, None) => start = Some(i),
            (true, Some(s)) => {
                out.push((s, i - 1));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s, lines.len() - 1));
    }
    out
}

/// The paragraph containing `line`, or `(line, line)` if it is blank.
pub fn active_paragraph(lines: &[String], line: usize) -> (usize, usize) {
    paragraphs(lines)
        .into_iter()
        .find(|&(s, e)| (s..=e).contains(&line))
        .unwrap_or((line, line))
}

/// First line of the next paragraph after the one containing `line`.
pub fn next_paragraph_start(lines: &[String], line: usize) -> Option<usize> {
    paragraphs(lines).into_iter().map(|(s, _)| s).find(|&s| s > line)
}

/// First line of the previous paragraph before the one containing `line`.
pub fn prev_paragraph_start(lines: &[String], line: usize) -> Option<usize> {
    let current = active_paragraph(lines, line).0;
    paragraphs(lines)
        .into_iter()
        .map(|(s, _)| s)
        .rev()
        .find(|&s| s < current.min(line))
}

/// Where the "current sentence" before `cursor` begins, for SHIFT+BACKSPACE.
///
/// A sentence runs from the last `.`/`?`/`!` (or the paragraph start) to the
/// cursor. A terminator sitting immediately before the cursor (ignoring
/// trailing whitespace) is treated as part of the sentence being deleted, so
/// deleting right after finishing a sentence removes that sentence rather
/// than nothing. Returns `None` when there is nothing to delete.
pub fn sentence_start_before(lines: &[String], cursor: Pos) -> Option<Pos> {
    let (cur_line, cur_col) = cursor;
    let para_start = active_paragraph(lines, cur_line).0;

    // Flatten the paragraph text up to the cursor into (position, char),
    // with byte-offset columns.
    let mut chars: Vec<(Pos, char)> = Vec::new();
    for (line_idx, line) in lines.iter().enumerate().take(cur_line + 1).skip(para_start) {
        let end = if line_idx == cur_line { cur_col } else { line.len() };
        for (col, ch) in line.char_indices() {
            if col >= end {
                break;
            }
            chars.push(((line_idx, col), ch));
        }
        if line_idx != cur_line {
            chars.push(((line_idx, end), '\n'));
        }
    }

    let is_term = |ch: char| matches!(ch, '.' | '?' | '!');
    let mut i = chars.len();
    while i > 0 && chars[i - 1].1.is_whitespace() {
        i -= 1;
    }
    // Skip terminator(s) directly before the cursor ("..." / "?!" count as one).
    while i > 0 && is_term(chars[i - 1].1) {
        i -= 1;
    }
    // Scan back to the previous sentence boundary.
    while i > 0 && !is_term(chars[i - 1].1) {
        i -= 1;
    }
    // The sentence begins after the boundary's trailing whitespace.
    let mut start = i;
    while start < chars.len() && chars[start].1.is_whitespace() {
        start += 1;
    }
    if start < chars.len() {
        Some(chars[start].0)
    } else if i < chars.len() {
        Some(chars[i].0) // only whitespace before the cursor: delete it
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(str::to_string).collect()
    }

    #[test]
    fn paragraphs_split_on_blank_lines() {
        let l = lines("one\ntwo\n\nthree\n\n\nfour");
        assert_eq!(paragraphs(&l), vec![(0, 1), (3, 3), (6, 6)]);
    }

    #[test]
    fn paragraph_navigation() {
        let l = lines("one\n\ntwo\n\nthree");
        assert_eq!(next_paragraph_start(&l, 0), Some(2));
        assert_eq!(next_paragraph_start(&l, 2), Some(4));
        assert_eq!(next_paragraph_start(&l, 4), None);
        assert_eq!(prev_paragraph_start(&l, 4), Some(2));
        assert_eq!(prev_paragraph_start(&l, 2), Some(0));
        assert_eq!(prev_paragraph_start(&l, 0), None);
    }

    #[test]
    fn sentence_start_mid_sentence() {
        let l = lines("First one. Second part here");
        assert_eq!(sentence_start_before(&l, (0, 27)), Some((0, 11)));
    }

    #[test]
    fn sentence_start_just_after_terminator_targets_whole_sentence() {
        let l = lines("First one. Second part. ");
        assert_eq!(sentence_start_before(&l, (0, 24)), Some((0, 11)));
    }

    #[test]
    fn sentence_start_stops_at_paragraph_start() {
        let l = lines("para one.\n\nno terminator here");
        assert_eq!(sentence_start_before(&l, (2, 18)), Some((2, 0)));
    }

    #[test]
    fn sentence_start_across_lines() {
        let l = lines("Stays. Sentence broken\nacross lines here");
        assert_eq!(sentence_start_before(&l, (1, 17)), Some((0, 7)));
    }

    #[test]
    fn sentence_start_nothing_to_delete() {
        let l = lines("");
        assert_eq!(sentence_start_before(&l, (0, 0)), None);
    }

    #[test]
    fn sentence_columns_are_byte_offsets() {
        // "héllo. wörld" — 'é' is 2 bytes, so "wörld" starts at byte 7.
        let l = lines("héllo. wörld");
        let cursor = (0, l[0].len());
        assert_eq!(sentence_start_before(&l, cursor), Some((0, 8)));
    }
}
