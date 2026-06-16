//! Editor decoration geometry: turning `(line, col)` byte spans into the
//! pixel rectangles the editor draws underlines / backgrounds over.
//!
//! The rectangles come out in **editor-local** coordinates — relative to the
//! text origin and already adjusted for the current scroll, exactly like the
//! widget's own selection rectangles — so the draw pass only adds the text
//! padding translation.
//!
//! The cosmic-text walk is a thin wrapper; the testable arithmetic (which
//! glyphs a byte span covers, and their horizontal extent) lives in
//! [`x_extent`].

use iced::advanced::graphics::text::cosmic_text;
use iced::Rectangle;

use crate::core::text::Pos;

/// Editor-local rectangles covering the byte span `[start, end)` across every
/// visual line it touches. A logical line that soft-wraps yields one rect per
/// wrapped run.
pub fn span_rects(buffer: &cosmic_text::Buffer, start: Pos, end: Pos) -> Vec<Rectangle> {
    let mut rects = Vec::new();
    for run in buffer.layout_runs() {
        let line = run.line_i;
        if line < start.0 || line > end.0 {
            continue;
        }
        // Byte sub-range of the span that lies on this logical line.
        let c0 = if line == start.0 { start.1 } else { 0 };
        let c1 = if line == end.0 { end.1 } else { usize::MAX };
        let glyphs = run.glyphs.iter().map(|g| (g.start, g.end, g.x, g.w));
        if let Some((x0, x1)) = x_extent(glyphs, c0, c1) {
            rects.push(Rectangle {
                x: x0,
                y: run.line_top,
                width: x1 - x0,
                height: run.line_height,
            });
        }
    }
    rects
}

/// The horizontal extent `(min_x, max_x)` of the glyphs whose byte range
/// `[start, end)` intersects `[c0, c1)`. Each glyph is `(start, end, x, w)`.
/// Returns `None` when no glyph is covered.
pub fn x_extent(
    glyphs: impl IntoIterator<Item = (usize, usize, f32, f32)>,
    c0: usize,
    c1: usize,
) -> Option<(f32, f32)> {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for (gs, ge, x, w) in glyphs {
        // Half-open overlap of [gs, ge) with [c0, c1).
        if gs < c1 && ge > c0 {
            lo = lo.min(x);
            hi = hi.max(x + w);
        }
    }
    (hi > lo).then_some((lo, hi))
}

#[cfg(test)]
mod tests {
    use super::*;

    // "abc def" → 7 glyphs, each 10px wide, laid left to right.
    fn glyphs() -> Vec<(usize, usize, f32, f32)> {
        (0..7).map(|i| (i, i + 1, i as f32 * 10.0, 10.0)).collect()
    }

    #[test]
    fn extent_covers_intersecting_glyphs() {
        // bytes [4, 7) → glyphs at x=40,50,60 → 40..70.
        assert_eq!(x_extent(glyphs(), 4, 7), Some((40.0, 70.0)));
    }

    #[test]
    fn extent_is_half_open() {
        // bytes [2, 3) → only the glyph at index 2 (x=20..30).
        assert_eq!(x_extent(glyphs(), 2, 3), Some((20.0, 30.0)));
        // A zero-width span covers nothing.
        assert_eq!(x_extent(glyphs(), 3, 3), None);
    }

    #[test]
    fn extent_clamps_to_available_glyphs() {
        // c1 past the end still bounds to the last glyph.
        assert_eq!(x_extent(glyphs(), 5, usize::MAX), Some((50.0, 70.0)));
    }

    #[test]
    fn extent_none_when_disjoint() {
        assert_eq!(x_extent(glyphs(), 20, 30), None);
    }
}
