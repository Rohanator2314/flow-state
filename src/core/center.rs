//! Pure arithmetic for typewriter centering.
//!
//! The editor scrolls only in whole **visual lines** (iced's
//! `Action::Scroll { lines }`), so each animation frame turns the pixel gap
//! between the active paragraph's middle and the viewport centre into a small,
//! eased number of lines to scroll. The buffer-dependent part — measuring that
//! gap from the cosmic layout — lives in the view/app layer; these two
//! functions are the unit-testable math it feeds.

/// Convert a vertical pixel offset into a whole number of visual lines to
/// scroll (rounded to the nearest line). Zero if the metrics are degenerate.
pub fn delta_lines(delta_px: f32, line_height: f32) -> i32 {
    if line_height <= 0.0 {
        return 0;
    }
    (delta_px / line_height).round() as i32
}

/// One eased step, in visual lines, that closes part of `delta` toward zero.
///
/// Ease-out: each call covers about half the remaining distance, capped at
/// `max_step` lines and never below one, so centering converges in a few frames
/// (≈150 ms at 60 fps) and always lands exactly on target. Returns 0 once the
/// gap is closed.
pub fn ease_step(delta: i32, max_step: i32) -> i32 {
    if delta == 0 {
        return 0;
    }
    let max_step = max_step.max(1);
    let magnitude = ((delta.abs() + 1) / 2).clamp(1, max_step);
    magnitude * delta.signum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_lines_rounds_to_nearest() {
        assert_eq!(delta_lines(0.0, 20.0), 0);
        assert_eq!(delta_lines(9.0, 20.0), 0); // < half a line → no scroll
        assert_eq!(delta_lines(11.0, 20.0), 1);
        assert_eq!(delta_lines(-50.0, 20.0), -3); // -2.5 rounds away from zero
        assert_eq!(delta_lines(100.0, 0.0), 0); // degenerate metrics
    }

    #[test]
    fn ease_step_converges_and_lands_exactly() {
        for &target in &[9, -9, 1, -1, 40] {
            let mut cur = 0;
            let mut steps = 0;
            while cur != target {
                let s = ease_step(target - cur, 100);
                assert!(s != 0 && s.signum() == (target - cur).signum());
                cur += s;
                // never overshoot
                assert!((target - cur).signum() == (target).signum() || target == cur);
                steps += 1;
                assert!(steps < 30, "did not converge for {target}");
            }
            assert_eq!(cur, target);
        }
    }

    #[test]
    fn ease_step_caps_and_zeroes() {
        assert_eq!(ease_step(100, 4), 4); // capped
        assert_eq!(ease_step(-100, 4), -4);
        assert_eq!(ease_step(0, 4), 0); // arrived
    }
}
