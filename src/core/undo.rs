//! Snapshot-based undo with typing-run coalescing.
//!
//! iced's `text_editor` has no built-in history, so the app keeps one
//! [`History`] per open file. Before each edit the app calls
//! [`History::record`] with the *pre-edit* state; consecutive printable
//! typing coalesces into a single step (whitespace breaks the run), giving
//! word-level undo granularity. Snapshots copy the whole text — prose-sized
//! files make that a non-issue; swap for an operation log behind this same
//! interface if it ever matters.

use crate::core::text::Pos;

/// A restorable editor state.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub text: String,
    pub cursor: Pos,
}

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// True while a run of ordinary typing is being coalesced into one step.
    typing_run: bool,
}

impl History {
    /// Record the state *before* an edit. `coalesce` is true for ordinary
    /// printable typing; any other edit starts a fresh undo step. A new edit
    /// always truncates the redo stack.
    pub fn record(&mut self, before: Snapshot, coalesce: bool) {
        if !(coalesce && self.typing_run) {
            self.undo.push(before);
        }
        self.typing_run = coalesce;
        self.redo.clear();
    }

    /// End the current typing run (e.g. on cursor movement) so the next
    /// keystroke starts a new undo step.
    pub fn break_run(&mut self) {
        self.typing_run = false;
    }

    /// Step back. `current` is the live state, pushed onto the redo stack.
    pub fn undo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let prev = self.undo.pop()?;
        self.redo.push(current);
        self.typing_run = false;
        Some(prev)
    }

    /// Step forward again after an undo.
    pub fn redo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let next = self.redo.pop()?;
        self.undo.push(current);
        self.typing_run = false;
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(text: &str, col: usize) -> Snapshot {
        Snapshot { text: text.to_string(), cursor: (0, col) }
    }

    /// Simulates the app: record pre-edit state, then "apply" the edit.
    #[test]
    fn coalesces_typing_runs_and_truncates_redo() {
        let mut h = History::default();
        let mut state = snap("", 0);
        let mut typed = String::new();
        for ch in "hello world".chars() {
            h.record(state.clone(), !ch.is_whitespace());
            typed.push(ch);
            state = snap(&typed, typed.chars().count());
        }
        assert_eq!(state.text, "hello world");

        // Word-level granularity: "world" → "hello " → "hello" → "".
        state = h.undo(state).unwrap();
        assert_eq!(state.text, "hello ");
        state = h.undo(state).unwrap();
        assert_eq!(state.text, "hello");
        state = h.undo(state).unwrap();
        assert_eq!(state.text, "");
        assert!(h.undo(state.clone()).is_none());

        state = h.redo(state).unwrap();
        assert_eq!(state.text, "hello");

        // A new edit truncates the redo stack.
        h.record(state.clone(), false);
        assert!(h.redo(snap("hello!", 6)).is_none());
    }

    #[test]
    fn break_run_splits_an_otherwise_coalesced_run() {
        let mut h = History::default();
        h.record(snap("", 0), true);
        h.break_run();
        h.record(snap("ab", 2), true);
        let s = h.undo(snap("abcd", 4)).unwrap();
        assert_eq!(s.text, "ab", "movement broke the typing run");
    }
}
