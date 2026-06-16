# Architecture

A map of the codebase for contributors. Product behavior is specified in
[spec.md](spec.md); this document is about how the code is put together.
Each module also carries a `//!` doc with its local details.

## The big picture

flow-state is an iced application (Elm architecture): a single `App` state,
a `Message` enum, `update(&mut App, Message) -> Task<Message>`, and a
stateless `view(&App) -> Element<Message>`. Slow work (LaTeX compiles) runs
off-thread via `Task::perform` and returns as a message.

```
            main.rs — iced::application(boot, update, view)
                         .subscription (window close, status TTL tick)
                  │
                  ▼
            app.rs ────────────────────────────────────────────┐
            │ App: docs: BTreeMap<DocId, Document>,            │
            │ active (focused doc), pane_grid::State, Sidebar  │
            │ update(Message) = the only place state changes   │
            └──────┬────────────────────────────┬──────────────┘
                   │ uses                       │ rendered by
                   ▼                            ▼
            core/ (UI-free, tested)       view/ (stateless)
            ├ text.rs    paragraphs,      ├ mod.rs       layout, status bar
            │            sentence scan    ├ sidebar.rs   directory tree
            ├ undo.rs    snapshot history ├ editor.rs    builds the editor
            ├ latex.rs   pdflatex→pdftoppm│              widget; keymap, dim hl,
            ├ center.rs  centering math   │              decorations, centering
            ├ theme.rs   halloy TOML→Color├ decoration.rs span→rect geometry
            ├ fonts.rs   system font list ├ widget/      forked text_editor
            └ config.rs  config.toml      │              (vendored, extended)
                                          ├ preview.rs   markdown / PDF pages
                                          ├ dialogs.rs   modals
                                          ├ menu.rs      ESC command bar
                                          └ style.rs     halloy widget styles
```

## Design rules

- **`core/` is UI-free.** Pure functions (or filesystem/subprocess at most),
  no widget or app-state knowledge, and it carries the unit-test suite. The
  one iced type allowed is `iced::Color` (what theme resolution produces).
- **Many documents, one keyed by pane.** Each open file is a `Document`
  (with its own content, undo history, and rendered `Preview`) stored in
  `App::docs` under a monotonic `DocId`; each `PaneKind::Editor(DocId)` pane
  renders one. `App::active` is the focused document — opening a file splits
  the active editor into a new pane (`open_file`/`spawn_editor`), and the
  single preview pane, status bar, and paragraph dimming all read the active
  document.
- **Focus moves through one door.** `App::set_focus(pane)` is the only place
  `focused`/`active` change together: it records the focused pane and, when
  that pane is an editor, makes its document active (focusing the preview
  leaves `active` on the last editor, so the preview keeps showing it). After
  any structural change (close, drag-swap), `validate_panes` re-establishes
  the invariants — every document has a live pane, `active` is a living
  editor, `focused` is a living pane — and drops documents whose pane is
  gone, so closing a pane is just `panes.close` + validate. The last editor
  never closes; the preview reopens on the next save.
- **The editor's text is owned by iced** (`text_editor::Content`), not by us.
  Custom operations (sentence delete, paragraph nav) work by reading lines
  out of the Content, computing a target position with `core::text`, and
  applying it back as a cursor move/selection + edit. Columns are **byte**
  offsets (cosmic-text convention) — `core::text` is written for that.
- **Undo is ours.** iced has no editor history; `app.rs` records a
  `core::undo` snapshot before every `Action::Edit` (coalescing typing runs)
  and restores Content + cursor on undo/redo.
- **Phantoms live inside the Content.** The stock `text_editor` renders only
  its own `Content`, so a "phantom" (the recall ghost of a deleted sentence)
  is real buffer text kept dimmed by the highlighter, tracked as
  `Document::phantom: Option<String>` (the not-yet-resolved ghost, sitting
  immediately after the cursor). It is therefore stripped on save, and dropped
  on undo/redo or abandon — it is never document content. See the phantom
  recipe below.
- **Key semantics live in one place**: `view/editor.rs::key_binding` maps
  key presses to `Binding`s/`Message`s; unclaimed keys fall through to the
  widget defaults. Dialogs are modal — when one is open, the view simply
  doesn't render interactive content underneath it.
- **Errors never crash the session.** Config/theme problems degrade to
  defaults plus a status message; compile errors open a dismissible dialog;
  closing with unsaved changes always prompts.

## Common changes, step by step

**Add or change a keybinding.** Edit `custom_binding` in `view/editor.rs`.
For a new behavior: add a `Message` variant, bind it
(`Binding::Custom(Message::…)`), handle it in `App::update`. Simple motions
or edits can often be expressed without a message at all via
`Binding::Sequence` of built-ins (see CTRL+BACKSPACE).

**Recolor editor text (dimming, ghost).** The `DimHighlighter`
(`view/editor.rs`) rides iced's highlighter hook — the only way to change glyph
*color*. Its `DimSettings` carry the active-paragraph line range and an optional
`ghost` span (the phantom, dimmed inside the active paragraph); `highlight_line`
splits each line at the ghost boundary. The view recomputes the settings each
frame; the widget re-runs the highlighter when they change. The `Format` is
color/font only — no underline/background.

**Underline / glow / background a span (decoration pass).** Anything beyond
color is a quad in the forked widget's decoration pass. `view/editor.rs` turns a
`(line, col)` byte span into editor-local rectangles via
`decoration::span_rects` (over the cosmic buffer read through
`Content::with_buffer`), wraps each in a `DecorationQuad { bounds, color, radius,
behind }`, and hands the `Vec` to `TextEditor::decorations(...)`. The widget's
`draw` translates each quad into widget space and fills it — `behind` ones
(glow) before the text, the rest (underlines) after. The CTRL/SHIFT accent
underline and the active-paragraph glow are both built this way.

**Typewriter centering.** Driven entirely from `App::update` (no widget state):
`request_center()` sets `App::centering` after any cursor move/edit (when
`typewriter_scroll` is on and the user hasn't hand-scrolled); that gates a
`window::frames()` subscription. Each `Message::CenterTick`,
`view::editor::center_step` reads the active paragraph's pixel midpoint from the
cosmic buffer (`Content::with_buffer`), and `core::center` converts the gap to an
eased whole-line `Action::Scroll`. It self-stops when centered or when the scroll
clamps (a no-progress guard). A manual wheel scroll arrives as
`Action::Scroll` in `Message::Edit`, which sets `user_scrolled` to suspend
centering until the next edit.

**React to held modifiers** (keybind hints, accent emphasis, PDF zoom). The
held-modifier set is tracked by the always-on `on_modifiers` subscription into
`App::modifiers`. Read it in `view/` to drive UI: `sidebar::keybind_hints`
picks its rows from it (and from whether a phantom is active), and
`editor::view` derives the emphasis span from it (CTRL → `text::word_before`,
SHIFT → `text::sentence_start_before`). There is no per-key plumbing — just
read `app.modifiers` wherever the UI should react.

**The phantom lifecycle** (deleted-sentence recall) is split between
`Message::DeleteSentence` (creates it: the sentence text is sliced out with
`text::slice`, stored in `Document::phantom`, and the cursor moved before it)
and the `Message::Edit` interception in `App::update`: while a phantom is
active, a matching `Insert` steps over the ghost (`Motion::Right`, no insert),
a non-matching `Insert` is performed normally (pushing the ghost right), and
any other action abandons it. `Document` owns the mechanics —
`phantom_discard`/`phantom_accept`/`phantom_trim_word` and the `delete_span`
helper — using `text::advance` to map the ghost string onto buffer positions.
TAB (`PhantomAccept`) and CTRL+BACKSPACE (`DeleteWord`) branch on the phantom
in their own arms. Anything that rebuilds the Content (`restore`) or persists
it (`save`) clears the phantom first.

**Add a preview for a new file type.** Add a `FileKind` variant and its
extension in `core/mod.rs`; produce the preview in `App::refresh_preview`
(sync like markdown, or via `Task::perform` like LaTeX); add a `Preview`
variant and render it in `view/preview.rs`. The compiler walks you through
the matches.

**Use another theme key.** flow-state reads themes in halloy's
surface-oriented format (`[general]`/`[text]`/`[buffer]`/…). To surface a new
color: add the key to the matching format struct in `core/theme.rs`
(`General`/`Text`/`Buffer`) as an `Option<Hex>`, add a field to the app-facing
`Theme`, and map it in `Styles::to_theme` with a `fallback()` default. Every
key is optional — unset colors must fall back, never blank. The IRC-specific
keys halloy ships (nicknames, server messages) are intentionally not modelled;
serde ignores them.

**Add a config option.** Add a defaulted field to `core/config.rs::Config`;
serde fills it when absent (it serializes back via `Config::save` for the
command bar). To expose it in the ESC command bar, add a `Command` variant
in `app.rs` (plus its `Display` label and `Command::ALL` entry) and handle
it in `App::run_command` — either apply it directly (like focus dimming) or
open a sub-view: a new `Menu` variant rendered in `view/menu.rs`, whose
confirm message mutates the config and calls `save_config`. To make it
hot-reload too, apply it in `App::poll_config` alongside theme/font/split.
Document it in the README example.

**Config hot-reload.** `App::poll_config` runs off the always-on 1 s `Tick`:
it compares the modification times of `config.toml` and the active theme file
(`config_signature`) against the last-seen `config_sig`, and on a change
re-reads the config and re-applies theme/font/split. It is a poll (not a
filesystem watcher) — no extra dependency, and immune to editor write/rename
event storms — at a cost of ≤1 s latency. It skips while the command bar is
open so it can't clobber the live theme/font preview, and `save_config`
refreshes the signature so the app's own writes don't round-trip.

**The command bar** (`view/menu.rs` + the `Menu`/`Picker`/`Command` types in
`app.rs`) is a plain `text_input` over a filtered option list — *not* iced's
`combo_box`, which cannot be focused programmatically. The filter text and
keyboard selection live in `Picker`; filtering is re-derived each update by
`filtered_commands`/`theme_options`/`compiler_options`. Arrow keys reach the
app through the keyboard subscription (the focused input ignores them),
ENTER through the input's `on_submit`. The theme sub-bar previews the
selected theme live by mutating `app.theme` only; the config is written when
a choice is confirmed.

**ESC behavior** is one handler (`Message::EscPressed`): it peels UI layers
in order — confirm dialog, error dialog, sub-view (back to the root bar,
reverting any theme/font preview), root bar (close, refocus editor) — and
opens the command bar when nothing is layered. ESC is delivered by an
`iced::event::listen_with` subscription (`app::on_escape`) rather than
`keyboard::listen`, because the command bar's focused `text_input`
*captures* ESC to unfocus itself; `listen` only sees uncaptured events, so
closing the bar would need two presses. `listen_with` sees every event
regardless of capture, giving one-press open/back/close. The editor keymap
deliberately does **not** bind ESC (that would double-fire).

**PDF preview scroll vs. zoom** (`view/preview.rs`): pages are stacked in a
`scrollable` for smooth page scrolling. Holding CTRL flips the wheel to
zoom — read from `App::modifiers` (kept current by the always-on
`on_modifiers` subscription). While held, the view wraps the pages in a
`mouse_area` whose `on_scroll` *captures* the wheel (so the scrollable does
not also move) and feeds `Message::PdfScroll`, which scales `App::pdf_zoom`.
The capture works because a `mouse_area` nested inside the scrollable
receives the event first.

**Add a font.** Fonts are not a fixed list — `core/fonts.rs` enumerates the
system's families via `fontdb` (the same library iced's text backend uses,
so every name resolves through `iced::Font::with_name`). The result is
cached. `app::resolve_font` maps a family name (or the empty/default
sentinel) to an `iced::Font`, leaking the name to `'static` as the API
requires.

**Add a pane kind.** Add a `PaneKind` variant, render it in `view/mod.rs`'s
pane closure, and create/close it in `App::sync_preview_pane` (or a sibling).

## Testing

- `cargo test` — unit tests live next to the code in `core/`. The LaTeX
  tests shell out to real `pdflatex`/`pdftoppm` (TeX Live + poppler
  required); the theme test parses the real `catppuccin_mocha.toml` at the
  repo root.
- The GUI itself is verified manually: `cargo run -- test.tex`, then check
  sidebar navigation (each opened file gets its own editor pane; the preview
  follows the focused one), typing/dimming, CTRL+S compile → scroll/zoom
  pages, pane drag/resize/maximize/close, and the unsaved-changes dialog on
  close.
- `cargo build` before manual testing — `cargo test` alone does **not**
  rebuild the binary, and testing a stale binary has bitten us before.

## Known limitations / future work

- The editor is a **fork of iced's `text_editor`** (`view/widget/text_editor.rs`,
  vendored MIT) because the stock widget can't do three things flow-state wants:
  a per-character **underline** (its highlighter `Format` is color/font only), a
  paragraph **glow**, and **scroll control** for typewriter centering. The fork
  keeps iced's fast integrated text rendering (`fill_editor`, cosmic-text cached
  layout — no per-frame reshaping); it only *adds* a decoration quad pass and a
  read-only `Content::with_buffer` accessor. Consequence: scroll is line-granular
  (`Action::Scroll { lines }`), so centering lands within ~half a line of exact
  center. A pixel-perfect alternative would mean hand-rendering glyphs (giving up
  the fast path) — deliberately not done.
- Themes load a subset of halloy's schema (the writing-app surfaces); halloy's
  light/dark dynamic pairs and multi-theme random selection are not supported.
- Sentence detection treats every `.?!` as a boundary (`e.g.` ends a
  sentence) — spec open question #1.
- The preview follows the focused editor — there is a single preview pane,
  not one per document. A previewable document keeps its rendered preview in
  its own `Document::preview`, so switching focus is instant.
- PDF preview rasterizes at most 50 pages (`core/latex.rs::MAX_PAGES`).
