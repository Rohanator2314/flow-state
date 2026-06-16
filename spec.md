# flow-state — Product Spec

## Overview

`flow-state` is a desktop writing application built with iced (Rust), designed
for focused, distraction-free writing. The layout is inspired by halloy: a
slim sidebar plus draggable, resizable panes. The core idea is that the active
paragraph is always in focus — full-color while surrounding paragraphs are
dimmed — pushing the writer to stay present in what they are currently
writing.

---

## Layout

```
┌──────────┬─────────────────────────────┬─────────────────────────────┐
│ 📁 dir   │ editor                    🗖 │ preview                   🗖 │
│ ▸ docs/  ├─────────────────────────────┼─────────────────────────────┤
│ ▾ notes/ │                             │   [ PDF page 1 ]            │
│   a.md   │  ...dim paragraph...        │   [ PDF page 2 ]  ← scroll  │
│   b.tex  │                             │   [ PDF page 3 ]  CTRL=zoom │
│          │  ACTIVE PARAGRAPH           │     or rendered markdown    │
│          │  cursor here                │                             │
│ new file…│                             │                             │
├──────────┴─────────────────────────────┴─────────────────────────────┤
│ file.tex ●   ⟳ compiling…                                    ¶ 2/14  │
└───────────────────────────────────────────────────────────────────────┘
```

- **Sidebar**: a "📁 Current directory:" header naming the working folder,
  then a collapsible directory tree of it (hidden files excluded). Clicking a
  file opens it in its **own editor pane** (already-open files are just
  refocused; a pristine empty scratch buffer is reused instead of leaving an
  empty pane). Open files are marked with a `•`, the focused one in the accent
  color. A "new file…" input at the bottom creates files. If the focused
  buffer is untitled, creating a file names that buffer — text written so far
  is carried over; otherwise the new file opens in its own pane.
- **Panes**: each open file is an editor pane; one shared preview pane sits
  alongside. They live in a pane grid with halloy-style gaps and rounded,
  focus-highlighted borders — drag a title bar to swap panes, drag the
  splitter to resize, 🗖 to maximize, ✕ to close (an editor closes once more
  than one is open; the preview closes and re-opens on the next save).
- **One preview, follows focus**: the preview pane shows the focused
  document's rendered markdown / compiled PDF, refreshing on that document's
  save. Focusing a plain-text document leaves the preview pane showing a
  hint. `.md`/`.tex` files open the preview at a configurable split ratio.
- **Status bar**: focused filename + unsaved dot, compile state, transient
  messages, and the cursor's paragraph / total paragraphs.

---

## Editor Behavior

### Mode
Always-insert (typewriter style). No modal editing; standard GUI text
editing (selection, clipboard, mouse) works as expected.

### Sections / Paragraphs
- A **section** is a paragraph: a block of text delimited by blank lines.
- Navigation moves between paragraphs, not headings.

### Focus Effect
- The **active paragraph** (the one containing the cursor) renders in the
  theme's full text color; all other paragraphs render dimmed
  (`ui.text.inactive`).
- *Future work*: vertically centering the active paragraph (typewriter
  scrolling). The stock iced `text_editor` does not expose scroll control;
  this needs a custom editor widget.

### Keybindings

| Key | Action |
|---|---|
| Arrows, HOME/END, CTRL+arrows, mouse | Standard movement/selection (widget defaults) |
| ALT+W / ALT+B | Move to the next / previous word |
| ALT+N / ALT+SHIFT+N | Jump to next / previous paragraph |
| CTRL+BACKSPACE | Delete the word before the cursor |
| SHIFT+BACKSPACE | Delete from the start of the current sentence to the cursor |
| CTRL+Z | Undo (full session history, unlimited depth) |
| CTRL+SHIFT+Z / CTRL+Y | Redo |
| CTRL+S | Save, then compile/refresh the preview |
| CTRL+C/X/V | Clipboard (widget defaults) |
| ESC | Open the command bar / back out one level; also dismisses dialogs |

Closing the window with unsaved changes opens a save / discard / cancel
dialog. (CTRL+E is reserved for a future fuzzy quick-open palette.)

### Command bar (ESC)

A halloy-style command bar, top-anchored over a dimmed backdrop: a filter
input above the matching options, `↑`/`↓` move the selection, ENTER (or a
click) confirms, ESC backs out one level (sub-view → root bar → closed).
Each root command leads to its setting:

- **theme** — a searchable sub-bar listing the built-in theme plus every
  theme in the themes directory. Moving the selection previews the theme
  live without touching the config; ENTER persists it, ESC reverts to the
  saved theme.
- **font** — a searchable sub-bar of the system's installed font families
  (queried via `fontdb`) plus the built-in default, for the editor typeface.
  Live-previews like the theme picker; ENTER persists, ESC reverts.
- **latex engine** — a sub-bar with the supported compilers (pdflatex,
  xelatex).
- **split width** — a slider panel for the editor/preview ratio; resizes the
  open split live, persists on release.
- **focus dimming** — toggles the dimming effect immediately and closes the
  bar.
- **help** — the keybinding reference above. Typing `?` in the root bar
  jumps straight to it.

Confirmed changes persist to config.toml. When the bar closes, focus returns
to the editor.

**SHIFT+BACKSPACE detail:** "Current sentence" is the text since the last
`.`, `?`, `!`, or paragraph start — whichever comes first before the cursor.
The deletion range is `[sentence_start, cursor)`. If the character
immediately before the cursor (ignoring trailing whitespace) is itself a
sentence terminator, it is skipped, so pressing SHIFT+BACKSPACE right after
finishing a sentence deletes that sentence rather than nothing.

### Undo
- Unlimited session history; consecutive printable typing coalesces into one
  step (whitespace breaks the run → word-level granularity); a new edit after
  undo truncates the redo stack. Per-file, in memory only.

---

## Preview Rendering

### LaTeX (`.tex`)
- On CTRL+S: save to disk, then run `pdflatex` or `xelatex` (TeX Live)
  against the saved file on a background task; the editor stays responsive
  and the status bar shows progress.
- The PDF is rasterized via `pdftoppm` (poppler-utils, `-png -r 144`, all
  pages capped at 50) and shown as a continuously scrollable column of page
  images — a plain wheel scrolls between pages, **CTRL+wheel** zooms (each
  page is scaled to the pane width times the zoom factor). The zoom level is
  kept across recompiles.
- Compile errors open a dismissible dialog quoting the first `!` line of the
  TeX log with context.

### Markdown (`.md`)
- On CTRL+S: save to disk, then re-render via iced's markdown widget
  (headings, emphasis, lists, code, quotes, links) — instant refresh.

### Plain text (anything else)
- No preview pane; the editor takes the full pane area. CTRL+S just saves.

---

## Configuration

### Location
`~/.config/flow-state/`

```
~/.config/flow-state/
├── config.toml        # main config
└── themes/            # halloy-format TOML theme files
```

### config.toml (example)
```toml
theme = "catppuccin_mocha"
latex_compiler = "pdflatex"   # or "xelatex"
preview_split_ratio = 0.5     # initial editor share of the pane area
focus_dimming = true          # the dimmed-paragraphs focus effect
editor_font = "JetBrains Mono"  # installed font family; empty = default sans
```

All options are editable live from the ESC command bar, which persists changes back
to this file. The config and the active theme file are also **hot-reloaded**:
external edits are picked up (within ~1 s) and applied without a restart.

### Themes
- [halloy](https://github.com/squidowl/halloy)'s surface-oriented TOML theme
  format; the community theme library at <https://themes.halloy.chat> drops
  straight into `~/.config/flow-state/themes/` (each theme page has a
  "Download TOML file" button).
- Colors are grouped by UI surface. flow-state reads: `buffer.background`
  (the writing canvas), `general.background` (chrome: sidebar/title/status),
  `general.border` (pane borders), `general.unread_indicator` (accent),
  `text.primary` (active text), `text.secondary` (dimmed text), and
  `text.success`/`error`/`warning`. Colors are `#rrggbb`/`#rrggbbaa` hex. The
  IRC-specific keys halloy themes also carry (nicknames, server messages) are
  ignored, so any halloy theme loads; any key flow-state needs but a file
  omits falls back to a neutral default (never blank).
- halloy's **Ferra** theme is bundled as the built-in default, so the app
  works with zero config.

---

## Tech Stack

| Concern | Choice |
|---|---|
| Language | Rust |
| GUI framework | iced 0.14 (`pane_grid`, `text_editor`, `markdown`, `image` widgets) |
| LaTeX compilation | System `pdflatex` / `xelatex` (subprocess) |
| PDF rasterization | System `pdftoppm` (poppler-utils, subprocess) |
| Markdown rendering | iced's built-in `markdown` widget |
| Config parsing | `toml` crate |
| Theme parsing | halloy's surface-oriented TOML theme format (`serde`) |

---

## Out of Scope (v1)

- Typewriter vertical centering of the active paragraph (needs a custom
  editor widget; see Focus Effect)
- Modal editing
- LSP / autocomplete
- Tabbed editing (files open as panes, not tabs)
- Fuzzy quick-open palette (sidebar tree covers v1)
- Git integration
- Spell check
- Custom keybinding configuration
- Light/dark dynamic theme pairs and multi-theme random selection (halloy
  features; flow-state uses a single configured theme)

---

## Open Questions

1. **Sentence boundary detection** for SHIFT+BACKSPACE: handle edge cases
   like `e.g.`, `Dr.`, abbreviations. (v1 treats every `.`/`?`/`!` as a
   boundary; a terminator immediately before the cursor is skipped.)
