---
name: flow-state-design
description: Design and visually refine Flow State's Rust/iced interface while preserving its quiet, writing-first character. Use for UI additions, interaction-state styling, layout changes, or visual review in this repository; do not invoke for behavior-only fixes with no visible interface impact.
---

# Flow State Design

Keep Flow State visually quiet: the document is the primary surface and application chrome should recede until it is useful. Extend the established design language rather than introducing a parallel component system.

## Read the Local Language

Before changing a view, inspect the nearby view code plus `src/view/style.rs` and `src/core/theme.rs`. Reuse an existing helper when it expresses the intended role. Add a narrowly named shared style helper only when the same interaction treatment recurs or stock iced styling clashes with the app.

Treat the resolved `Theme` fields as semantic tokens:

- `background`: editor canvas; keep it dominant.
- `surface`: sidebar, bars, menus, and dialogs.
- `text`: primary labels and current values.
- `text_inactive` or `surface_text`: hints, metadata, and secondary labels.
- `accent`: current selection, focus, and the single strongest non-destructive emphasis.
- `border`: quiet separation; prefer it to extra layers or decoration.
- `danger`: destructive labels and confirmation emphasis.

Do not introduce hard-coded product colors where one of these tokens fits. Transparency derived from a token or neutral black/white is appropriate for hover, pressed, shadow, and backdrop states.

## Compose the Hierarchy

- Preserve compact density in navigation and utility surfaces. Start from the local row heights, type sizes, 4 px corner radius, and small spacing increments already in use.
- Give each surface one clear primary cue. A context menu needs a target label and scannable actions; a chooser needs a title, brief explanation, and visibly distinct choices.
- Use spacing and muted text before adding dividers, shadows, or new colors. Floating dialogs and command surfaces may use a soft shadow; inline sidebar panels usually need only a border and tonal separation.
- Keep labels plain and specific. Icons or symbols may aid scanning, but never make them the sole representation of an action, and verify that the bundled font can render them.
- Preserve the editor's available space. Avoid widening persistent chrome or adding always-visible controls unless the task requires them.

## Design Interaction States

Interactive elements need discernible idle, hover, pressed, focused/selected, and disabled states when those states can occur. Keep hover changes subtle; reserve accent treatment for focus or selection. Maintain readable contrast across every bundled/custom theme rather than tuning only for the default dark theme.

For compact action rows, align labels consistently, make the full row clickable, and use a stable hit target even when typography is small. Inputs should clearly belong to the operation they complete, with primary and cancel actions grouped nearby.

Destructive actions must be visually distinguishable without dominating the initial menu. Use `danger` for the destructive action label or state, require explicit confirmation for recursive or irreversible deletion, state the affected object in the confirmation copy, and place the safe cancel action alongside it.

## Refine Without Expanding Scope

Preserve behavior, keyboard navigation, focus restoration, and accessibility semantics while styling. Do not redesign unrelated screens merely for consistency. When a desired visual treatment would require new application state or behavioral changes, keep it separate and explain the tradeoff instead of smuggling it into a cosmetic edit.

## Verify the Result

Run formatting, linting, and the relevant tests required by the repository. Then inspect each changed surface at its meaningful states: idle, hover/focus where feasible, data-entry, validation/error, and destructive confirmation. Check long names, narrow sidebar width, and at least one alternate theme when practical. If the desktop UI cannot be rendered in the environment, say so and perform a source-level review of spacing, token use, text hierarchy, and state coverage rather than claiming visual verification.
