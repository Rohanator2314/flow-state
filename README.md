# flow-state

A distraction-free writing app. The paragraph you are writing stays in full
color; everything else is dimmed. Built with [iced](https://iced.rs) —
a halloy-style layout with a directory sidebar and draggable, resizable
editor/preview panes.

For `.tex` and `.md` files a preview pane opens next to the editor — LaTeX is
compiled and shown as a continuously scrollable column of page images
(scroll through pages, **CTRL+scroll** to zoom), markdown is rendered
natively. Everything else is plain text, full width.

## Requirements

- A GPU-capable system (iced renders via wgpu; falls back to software
  rendering where unavailable).
- For LaTeX preview: `pdflatex` (or `xelatex`) and `pdftoppm` (poppler-utils)
  on `$PATH`. 

## Usage

```sh
cargo run --release -- notes.md     # open a file
cargo run --release --              # start with an untitled buffer
```

Clicking a file in the sidebar opens it in its own editor pane (so opening
files doesn't replace what you're working on); clicking an already-open file
just refocuses it. The preview pane follows whichever editor is focused.
Create files with the "new file…" input at the sidebar's bottom — if the
focused buffer is the untitled scratch one, that buffer is named and nothing
you wrote is lost. Drag pane title bars to swap panes, drag the splitter to
resize, 🗖 to maximize, ✕ to close a pane.

## Keybindings

| Key | Action |
|---|---|
| Arrows / CTRL+arrows / mouse | Standard movement & selection |
| `ALT+W` / `ALT+B` | Next / previous word |
| `ALT+N` / `ALT+SHIFT+N` | Next / previous paragraph |
| `CTRL+BACKSPACE` | Delete previous word (or trim a phantom's last word) |
| `SHIFT+BACKSPACE` | "Delete" the current sentence into a phantom |
| `TAB` | Accept the active phantom (else insert a tab) |
| `CTRL+Z` / `CTRL+SHIFT+Z` / `CTRL+Y` | Undo / redo |
| `CTRL+S` | Save, then refresh/compile the preview |
| `CTRL+C/X/V` | Clipboard |
| `ESC` | Open the command bar / go back / close |

The sidebar shows a live keybind cheat-sheet above the new-file input that
changes with the modifier you hold (CTRL/SHIFT/ALT); while a key is held, the
editor highlights — in the accent color — the word or sentence its BACKSPACE
would delete.

**Phantoms.** `SHIFT+BACKSPACE` doesn't hard-delete the sentence — it leaves a
dimmed "phantom" of it after the cursor. Type the same letters to fill it back
in (different letters push it along), `TAB` to accept it, `SHIFT+BACKSPACE`
again to discard it, or `CTRL+BACKSPACE` to drop its last word. Phantoms are
never saved to disk.

Closing the window with unsaved changes asks save / discard / cancel.

## Command bar (ESC)

Press ESC for a halloy-style command bar: type to filter, `↑`/`↓` to select,
ENTER to confirm, ESC to go back a level. Each command leads to its
setting:

- **theme** — a searchable list of every installed theme; arrowing through
  it previews each theme **live**, ENTER keeps it (ESC reverts).
- **font** — a searchable list of the system's installed fonts for the
  editor, also with live preview.
- **latex engine** — pdflatex or xelatex.
- **split width** — a slider for the editor/preview ratio (applies live).
- **focus dimming** — toggles the paragraph-dimming effect.
- **help** — the keybinding reference; typing `?` in the bar jumps straight
  there.

Changes apply immediately and are saved to `~/.config/flow-state/config.toml`
(note: hand-written comments in that file don't survive a save from the
menu).

## Configuration

Optional, at `~/.config/flow-state/config.toml`:

```toml
theme = "catppuccin_mocha"    # name of a file in ~/.config/flow-state/themes/
latex_compiler = "pdflatex"   # or "xelatex"
preview_split_ratio = 0.5     # initial editor share of the pane area
focus_dimming = true          # dim paragraphs outside the active one
editor_font = "JetBrains Mono"  # installed font family; empty = default sans
```

All of these are also editable live from the ESC command bar. The file is
**hot-reloaded**: edit `config.toml` (or the active theme file) in any editor
and the changes apply within about a second — no restart needed.

Themes use [halloy](https://github.com/squidowl/halloy)'s TOML theme format,
so the community theme library at <https://themes.halloy.chat> works directly:
hit "Download TOML file" on any theme and drop it into
`~/.config/flow-state/themes/`, then select it from the ESC command bar (or set
`theme` in the config). flow-state reads the surfaces it needs from the file
(`buffer.background`, `general.background`/`border`/`unread_indicator`,
`text.primary`/`secondary`/`success`/`error`/`warning`) and ignores the
IRC-specific keys; any surface a theme omits falls back to a neutral default.
With no config at all, halloy's bundled **Ferra** theme is used.

## License

GPL-3.0 (see [LICENSE](LICENSE)). The pane chrome (title bars, controls,
gaps, focus borders) and the theme format/parser are adapted from
[halloy](https://github.com/squidowl/halloy) (GPL-3.0); the bundled **Ferra**
theme (`assets/themes/ferra.toml`) is halloy's default theme by
[Casper Storm](https://github.com/casperstorm/ferra).

## Contributing

The product behavior is specified in [spec.md](spec.md); the code layout,
data flow, and step-by-step recipes for common changes (new keybinding, new
preview type, new pane kind, new config/theme key) are in
[ARCHITECTURE.md](ARCHITECTURE.md). `cargo test` runs the unit suite — the
LaTeX pipeline tests need `pdflatex` and `pdftoppm` installed.
