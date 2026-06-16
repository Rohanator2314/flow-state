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

/// The byte range `[start, cursor)` on the cursor's line that a "delete
/// previous word" (CTRL+BACKSPACE) would remove — i.e. the word the cursor
/// sits after, including the whitespace between it and the cursor. Used to
/// highlight that word while CTRL is held. Stays within the current line (the
/// highlight is a hint); returns `None` if there is no word before the cursor
/// on this line.
pub fn word_before(lines: &[String], cursor: Pos) -> Option<(Pos, Pos)> {
    let (line, col) = cursor;
    let text = lines.get(line)?;
    let col = col.min(text.len());
    let chars: Vec<(usize, char)> = text[..col].char_indices().collect();
    let mut i = chars.len();
    while i > 0 && chars[i - 1].1.is_whitespace() {
        i -= 1;
    }
    let word_end = i;
    while i > 0 && !chars[i - 1].1.is_whitespace() {
        i -= 1;
    }
    if i == word_end {
        return None; // only whitespace (or nothing) before the cursor
    }
    let start = chars[i].0;
    Some(((line, start), (line, col)))
}

/// Byte offset just past the first word of `s` and its trailing whitespace —
/// the span CTRL+BACKSPACE removes from the *front* of a phantom (the word
/// closest to the cursor, since the ghost sits just after it). Returns
/// `s.len()` when there is no following word.
pub fn first_word_end(s: &str) -> usize {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut i = 0;
    while i < chars.len() && chars[i].1.is_whitespace() {
        i += 1; // leading whitespace
    }
    while i < chars.len() && !chars[i].1.is_whitespace() {
        i += 1; // the word itself
    }
    while i < chars.len() && chars[i].1.is_whitespace() {
        i += 1; // trailing whitespace, so the next word becomes flush
    }
    chars.get(i).map(|(b, _)| *b).unwrap_or(s.len())
}

/// The text between two positions `[a, b)`, with `\n` rejoining lines. Used to
/// capture a deleted sentence as a phantom.
pub fn slice(lines: &[String], a: Pos, b: Pos) -> String {
    let ((al, ac), (bl, bc)) = (a, b);
    if al == bl {
        return lines
            .get(al)
            .and_then(|l| l.get(ac..bc))
            .unwrap_or("")
            .to_string();
    }
    let mut out = String::new();
    if let Some(l) = lines.get(al) {
        out.push_str(l.get(ac..).unwrap_or(""));
    }
    out.push('\n');
    for line in lines.get(al + 1..bl).unwrap_or(&[]) {
        out.push_str(line);
        out.push('\n');
    }
    if let Some(l) = lines.get(bl) {
        out.push_str(l.get(..bc).unwrap_or(""));
    }
    out
}

/// The position reached by walking `s` forward from `start`, advancing the
/// line on each `\n` and the byte column otherwise. Used to find where a
/// phantom (a run of text sitting just after the cursor) ends.
pub fn advance(start: Pos, s: &str) -> Pos {
    let (mut line, mut col) = start;
    for ch in s.chars() {
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += ch.len_utf8();
        }
    }
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(str::to_string).collect()
    }

    #[test]
    fn word_before_picks_the_preceding_word() {
        let l = lines("hello world");
        // Cursor at end: the word "world" (no trailing ws) is [6, 11).
        assert_eq!(word_before(&l, (0, 11)), Some(((0, 6), (0, 11))));
        // Cursor after "hello " (col 6): "hello" plus the trailing space.
        assert_eq!(word_before(&l, (0, 6)), Some(((0, 0), (0, 6))));
        // Start of line: nothing before.
        assert_eq!(word_before(&l, (0, 0)), None);
    }

    #[test]
    fn first_word_end_spans_first_word_and_trailing_space() {
        // "delete this sentence" → past "delete " → start of "this" (byte 7).
        assert_eq!(first_word_end("delete this sentence"), 7);
        // Single word (no following word) → whole string.
        assert_eq!(first_word_end("word"), 4);
        assert_eq!(first_word_end("word "), 5);
        // Leading whitespace is skipped before the word.
        assert_eq!(first_word_end("  a b"), 4); // past "  a " → 'b' at byte 4
        assert_eq!(first_word_end("   "), 3);
    }

    #[test]
    fn slice_extracts_spans() {
        let l = lines("First one. Second part here");
        assert_eq!(slice(&l, (0, 11), (0, 27)), "Second part here");
        let m = lines("Stays. Sentence broken\nacross lines here");
        assert_eq!(slice(&m, (0, 7), (1, 17)), "Sentence broken\nacross lines here");
    }

    #[test]
    fn advance_walks_over_newlines() {
        assert_eq!(advance((0, 3), "ab"), (0, 5));
        assert_eq!(advance((0, 3), "a\nbc"), (1, 2));
        assert_eq!(advance((2, 0), "one\ntwo\n"), (4, 0));
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
