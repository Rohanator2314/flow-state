//! Application state and update logic (Elm architecture).
//!
//! [`App`] owns everything mutable: the open [`Document`]s (one per editor
//! pane), the pane layout, the sidebar, and any open dialog. [`App::update`]
//! is the single place state changes; [`crate::view`] renders it. Slow work
//! (LaTeX compiles) runs off-thread via [`Task::perform`] and comes back as a
//! [`Message::Compiled`].
//!
//! Multiple files can be open at once: each lives in its own editor pane,
//! keyed by [`DocId`]. The single preview pane follows the focused editor
//! ([`App::active`]) — it renders that document's preview, status bar, and
//! paragraph dimming.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

use iced::widget::{image, markdown, pane_grid};
use iced::{Element, Subscription, Task, Theme, window};

use crate::view::widget::text_editor;

use crate::core::config::Config;
use crate::core::spell::{LoadedDictionary, SpellIssue};
use crate::core::theme::Theme as FlowTheme;
use crate::core::undo::{History, Snapshot};
use crate::core::{self, FileKind, file_kind, text};
use crate::view::{
    self,
    sidebar::{self, ContextMode, Sidebar},
};

/// How long transient status-bar messages stay visible.
const STATUS_TTL: Duration = Duration::from_secs(4);
const SPELL_DEBOUNCE: Duration = Duration::from_millis(350);

/// Identifies an open document (and so its editor pane). Monotonic — never
/// reused, so a stale id simply finds nothing.
pub type DocId = usize;

#[derive(Debug, Clone)]
pub enum Message {
    // editor (an edit targets a specific pane's document)
    Edit(DocId, text_editor::Action),
    Save,
    Undo,
    Redo,
    DeleteSentence,
    DeleteWord,
    PhantomAccept,
    NextParagraph,
    PrevParagraph,
    // spell checking/correction
    SpellDictionaryLoaded(u64, Result<LoadedDictionary, String>),
    SpellTick,
    SpellChecked(DocId, u64, Vec<SpellIssue>),
    OpenSpellCorrection,
    SpellSuggestions(DocId, u64, Vec<String>),
    SpellCorrectionInput(String),
    SpellCorrectionPrev,
    SpellCorrectionNext,
    SpellCorrectionSubmit,
    SpellCorrectionApply(String),
    SpellCorrectionIgnore,
    AddSpellWord,
    SpellWordSaved(u64, String, Result<(), String>),
    CloseSpellCorrection,
    /// Held-modifier set changed — drives the sidebar keybind hints, the
    /// accent emphasis, and PDF wheel zoom.
    ModifiersChanged(iced::keyboard::Modifiers),
    /// One animation frame of typewriter centering.
    CenterTick,
    /// The window gained focus — re-focus the active editor (see
    /// [`on_window_focused`]).
    WindowFocused,
    // preview
    Compiled(DocId, Result<Vec<PdfPage>, String>),
    PdfScroll(iced::mouse::ScrollDelta),
    DismissError,
    LinkClicked(markdown::Uri),
    // panes
    PaneDragged(pane_grid::DragEvent),
    PaneResized(pane_grid::ResizeEvent),
    PaneClicked(pane_grid::Pane),
    ToggleMaximize(pane_grid::Pane),
    ClosePane(pane_grid::Pane),
    // sidebar
    ToggleSidebar,
    ToggleDir(PathBuf),
    ChangeDirectory(PathBuf),
    OpenFile(PathBuf),
    OpenSidebarContext(PathBuf, bool),
    CloseSidebarContext,
    SidebarContextCreateFile,
    SidebarContextCreateFolder,
    SidebarContextRename,
    SidebarContextDelete,
    SidebarContextConfirmDelete,
    SidebarContextInput(String),
    SidebarContextSubmit,
    NewFileInput(String),
    CreateFile,
    // quality-of-life keybinds
    /// CTRL+N — open a fresh untitled scratch pane.
    NewFile,
    /// CTRL+O — choose a file to open or a folder to make current.
    OpenFilePicker,
    ChooseFileToOpen,
    ChooseFolderToOpen,
    CloseOpenPicker,
    FilePicked(Option<PathBuf>),
    FolderPicked(Option<PathBuf>),
    /// CTRL+W — close the focused pane (confirming if it has unsaved changes).
    CloseActivePane,
    /// CTRL+TAB / CTRL+SHIFT+TAB — move focus to the next / previous pane.
    NextPane,
    PrevPane,
    // in-pane find (CTRL+F)
    /// CTRL+F: open the find bar, or close it if already open.
    ToggleSearch,
    SearchInput(String),
    SearchNext,
    SearchPrev,
    CloseSearch,
    // command bar (the ESC menu)
    EscPressed,
    CommandInput(String),
    MenuPrev,
    MenuNext,
    MenuSubmit,
    CommandSelected(Command),
    ThemeSelected(String),
    CompilerSelected(String),
    FontSelected(String),
    SplitRatioChanged(f32),
    SplitRatioReleased,
    // window / dialogs
    CloseRequested,
    ConfirmSave,
    ConfirmDiscard,
    ConfirmCancel,
    Tick,
}

/// The ESC menu: a halloy-style command bar. The root lists commands; most
/// drill into a sub-bar (theme/compiler pickers) or a small panel
/// (split slider, keybind help).
pub enum Menu {
    Commands(Picker),
    Theme(Picker),
    Compiler(Picker),
    Font(Picker),
    Split,
    Help,
}

/// One command-bar level: what's typed in the filter input and which row
/// the arrow keys have selected (an index into the *filtered* options).
#[derive(Debug, Default)]
pub struct Picker {
    pub input: String,
    pub selected: usize,
}

/// The CTRL+F find bar's state: the query, every match in the focused
/// document (as `[start, end)` position spans), and which one is current.
#[derive(Default)]
pub struct Search {
    pub query: String,
    pub matches: Vec<(text::Pos, text::Pos)>,
    pub current: Option<usize>,
    /// The cursor position when the bar opened; incremental matching anchors to
    /// it (so typing more letters doesn't walk the selection forward).
    origin: text::Pos,
}

/// The keyboard-first correction chooser opened by CTRL+.
pub struct SpellCorrection {
    pub doc_id: DocId,
    pub revision: u64,
    pub issue: SpellIssue,
    pub input: String,
    pub suggestions: Vec<String>,
    pub selected: usize,
    pub loading: bool,
}

/// Root command-bar entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Theme,
    Font,
    Compiler,
    Split,
    Dimming,
    Typewriter,
    Glow,
    Spelling,
    Help,
}

impl Command {
    const ALL: [Command; 9] = [
        Command::Theme,
        Command::Font,
        Command::Compiler,
        Command::Split,
        Command::Dimming,
        Command::Typewriter,
        Command::Glow,
        Command::Spelling,
        Command::Help,
    ];
}

/// Case-insensitive substring match, the command bar's filter rule.
fn matches(option: &str, input: &str) -> bool {
    option
        .to_lowercase()
        .contains(input.trim().to_lowercase().as_str())
}

/// Root commands matching the filter input.
pub fn filtered_commands(input: &str) -> Vec<Command> {
    Command::ALL
        .into_iter()
        .filter(|c| matches(&c.to_string(), input))
        .collect()
}

/// Theme names matching the filter input.
pub fn theme_options(input: &str) -> Vec<String> {
    core::config::available_themes()
        .into_iter()
        .filter(|name| matches(name, input))
        .collect()
}

/// LaTeX compilers matching the filter input.
pub fn compiler_options(input: &str) -> Vec<String> {
    ["pdflatex", "xelatex"]
        .into_iter()
        .filter(|name| matches(name, input))
        .map(str::to_string)
        .collect()
}

/// Font families matching the filter input, with the built-in default first.
pub fn font_options(input: &str) -> Vec<String> {
    std::iter::once(core::config::BUILTIN_THEME.to_string())
        .chain(core::fonts::available().iter().cloned())
        .filter(|name| matches(name, input))
        .collect()
}

/// Resolve a theme by its command-bar name — used for the live preview while
/// arrowing through the theme list, without touching the config.
fn load_theme_by_name(name: &str) -> FlowTheme {
    if name == core::config::BUILTIN_THEME {
        return FlowTheme::default();
    }
    core::config::config_dir()
        .map(|d| d.join("themes").join(format!("{name}.toml")))
        .and_then(|path| FlowTheme::load(&path).ok())
        .unwrap_or_default()
}

/// Subscription filter: ESC, regardless of whether a widget captured it.
fn on_escape(
    event: iced::Event,
    _status: iced::event::Status,
    _window: window::Id,
) -> Option<Message> {
    use iced::keyboard::{Event, Key, key::Named};
    matches!(
        event,
        iced::Event::Keyboard(Event::KeyPressed {
            key: Key::Named(Named::Escape),
            ..
        })
    )
    .then_some(Message::EscPressed)
}

/// Subscription filter: arrow keys drive the command-bar selection.
fn on_menu_arrows(
    event: iced::Event,
    _status: iced::event::Status,
    _window: window::Id,
) -> Option<Message> {
    use iced::keyboard::{Event, Key, key::Named};
    let iced::Event::Keyboard(Event::KeyPressed { key, .. }) = event else {
        return None;
    };
    match key {
        Key::Named(Named::ArrowUp) => Some(Message::MenuPrev),
        Key::Named(Named::ArrowDown) => Some(Message::MenuNext),
        _ => None,
    }
}

/// Subscription filter: up/down choose a spelling suggestion while the
/// correction input owns keyboard focus.
fn on_spell_arrows(
    event: iced::Event,
    _status: iced::event::Status,
    _window: window::Id,
) -> Option<Message> {
    use iced::keyboard::{Event, Key, key::Named};
    let iced::Event::Keyboard(Event::KeyPressed { key, .. }) = event else {
        return None;
    };
    match key {
        Key::Named(Named::ArrowUp) => Some(Message::SpellCorrectionPrev),
        Key::Named(Named::ArrowDown) => Some(Message::SpellCorrectionNext),
        _ => None,
    }
}

/// Subscription filter: track the held-modifier set (keybind hints, accent
/// emphasis, PDF wheel zoom).
fn on_modifiers(
    event: iced::Event,
    _status: iced::event::Status,
    _window: window::Id,
) -> Option<Message> {
    use iced::keyboard::Event;
    match event {
        iced::Event::Keyboard(Event::ModifiersChanged(m)) => Some(Message::ModifiersChanged(m)),
        _ => None,
    }
}

/// Subscription filter: CTRL+F toggles the find bar. Global (not an editor
/// keybind) so it fires regardless of which widget has focus — in particular it
/// can *close* the bar, when the find input is focused and the editor's keymap
/// would never run.
fn on_find_toggle(
    event: iced::Event,
    _status: iced::event::Status,
    _window: window::Id,
) -> Option<Message> {
    use iced::keyboard::{Event, Key};
    match event {
        iced::Event::Keyboard(Event::KeyPressed { key, modifiers, .. })
            if modifiers.control() && matches!(key.as_ref(), Key::Character("f")) =>
        {
            Some(Message::ToggleSearch)
        }
        _ => None,
    }
}

/// Subscription filter: the window gaining focus. We re-focus the active editor
/// on this so its caret is visible immediately — at launch the boot-time focus
/// can be lost if the window is created before the WM gives it focus, leaving
/// the cursor invisible until the first click. The handler ignores it while an
/// overlay (command bar, find bar, dialog) owns focus.
fn on_window_focused(
    event: iced::Event,
    _status: iced::event::Status,
    _window: window::Id,
) -> Option<Message> {
    matches!(event, iced::Event::Window(window::Event::Focused)).then_some(Message::WindowFocused)
}

/// Resolve a font-family name to an `iced::Font`. The built-in sentinel (and
/// an empty name) map to the default sans-serif. A named family is leaked to
/// `'static` so it can live in `Font::with_name`; the font set is small and
/// this happens only on selection, so the leak is bounded.
fn resolve_font(name: &str) -> iced::Font {
    if name.is_empty() || name == core::config::BUILTIN_THEME {
        iced::Font::DEFAULT
    } else {
        iced::Font::with_name(Box::leak(name.to_string().into_boxed_str()))
    }
}

impl std::fmt::Display for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Command::Theme => "theme — switch color theme",
            Command::Font => "font — editor typeface",
            Command::Compiler => "latex engine — choose the compiler",
            Command::Split => "split width — editor/preview ratio",
            Command::Dimming => "focus dimming — toggle paragraph dimming",
            Command::Typewriter => "typewriter scroll — center the active paragraph",
            Command::Glow => "paragraph glow — glow the active paragraph",
            Command::Spelling => "spell checking — toggle local corrections",
            Command::Help => "help — keybindings (?)",
        })
    }
}

/// What kind of content a pane shows.
/// What a pane shows. Each editor pane carries the [`DocId`] of the document
/// it renders; there is at most one preview pane (it follows the focus).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneKind {
    Editor(DocId),
    Preview,
}

/// The open file: its text (owned by iced's editor), undo history, path, and
/// its own preview/compile state (so each document keeps its rendered preview
/// even while another is focused).
pub struct Document {
    pub path: Option<PathBuf>,
    pub content: text_editor::Content,
    pub history: History,
    pub modified: bool,
    /// Bumped whenever `content` is replaced wholesale (undo/redo), so the
    /// dimming highlighter knows its cached lines are stale.
    pub generation: usize,
    /// This document's rendered preview (markdown / PDF pages), shown when it
    /// is the focused document.
    pub preview: Preview,
    /// A LaTeX compile is running for this document.
    pub compiling: bool,
    /// The last compile error for this document, shown as a modal while it is
    /// focused.
    pub compile_error: Option<String>,
    /// A "phantom" of a just-deleted sentence: the deleted text, kept dimmed in
    /// the buffer immediately after the cursor. The writer can type it back
    /// (matching chars fill in, others push it along), TAB to accept it, or
    /// SHIFT/CTRL+BACKSPACE to discard it (whole, or the last word). `None`
    /// when no phantom is active. Stripped from the buffer on save.
    pub phantom: Option<String>,
    /// Derived spelling diagnostics for the current real document text.
    pub spell_issues: Vec<SpellIssue>,
    /// Bumped for every text mutation; gates asynchronous scan/suggestion
    /// results so byte spans from an older buffer are never applied.
    spell_revision: u64,
    /// When this document last became eligible for a debounced scan.
    spell_dirty_since: Option<Instant>,
    /// A scan is currently running for some revision of this document.
    spell_in_flight: bool,
    /// The buffer text as last saved or loaded — the baseline `modified` is
    /// measured against, so reverting edits (or undoing to the saved state)
    /// clears the unsaved marker. See [`Document::refresh_modified`].
    saved_text: String,
}

impl Document {
    fn untitled() -> Self {
        Self {
            path: None,
            content: text_editor::Content::new(),
            history: History::default(),
            modified: false,
            generation: 0,
            preview: Preview::None,
            compiling: false,
            compile_error: None,
            phantom: None,
            spell_issues: Vec::new(),
            spell_revision: 0,
            spell_dirty_since: None,
            spell_in_flight: false,
            saved_text: String::new(),
        }
    }

    fn open(path: PathBuf) -> Self {
        let content = match std::fs::read_to_string(&path) {
            Ok(text) => text_editor::Content::with_text(&text),
            Err(_) => text_editor::Content::new(),
        };
        let saved_text = content.text();
        let mut doc = Self {
            path: Some(path),
            content,
            history: History::default(),
            modified: false,
            generation: 0,
            preview: Preview::None,
            compiling: false,
            compile_error: None,
            phantom: None,
            spell_issues: Vec::new(),
            spell_revision: 0,
            spell_dirty_since: None,
            spell_in_flight: false,
            saved_text,
        };
        // `with_text` leaves the cursor — and the viewport — at the end of the
        // text; jump to the very start so the document opens showing its
        // beginning. DocumentStart scrolls the view to the top too, which a
        // bare cursor move would not.
        doc.content.perform(text_editor::Action::Move(
            text_editor::Motion::DocumentStart,
        ));
        doc
    }

    pub fn kind(&self) -> FileKind {
        file_kind(self.path.as_deref())
    }

    /// A fresh, never-edited [untitled] scratch buffer — safe to replace when
    /// opening a file, since it holds nothing the user typed.
    fn is_pristine(&self) -> bool {
        self.path.is_none() && !self.modified && self.content.text().trim().is_empty()
    }

    pub fn display_name(&self) -> String {
        self.path
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "[untitled]".to_string())
    }

    fn snapshot(&self) -> Snapshot {
        let pos = self.content.cursor().position;
        Snapshot {
            text: self.content.text(),
            cursor: (pos.line, pos.column),
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        // The content is rebuilt wholesale, so any phantom's positions are
        // void — drop it (the snapshot text already holds the solid sentence).
        self.phantom = None;
        self.content = text_editor::Content::with_text(&snapshot.text);
        self.move_to(snapshot.cursor);
        self.modified = true;
        self.generation += 1;
    }

    fn move_to(&mut self, (line, column): text::Pos) {
        self.content.move_to(text_editor::Cursor {
            position: text_editor::Position { line, column },
            selection: None,
        });
    }

    /// The cursor as a `(line, byte_col)` position.
    fn cursor_pos(&self) -> text::Pos {
        let p = self.content.cursor().position;
        (p.line, p.column)
    }

    /// Select `[a, b)` and delete it, leaving the cursor at `a`.
    fn delete_span(&mut self, a: text::Pos, b: text::Pos) {
        self.content.move_to(text_editor::Cursor {
            position: text_editor::Position {
                line: a.0,
                column: a.1,
            },
            selection: Some(text_editor::Position {
                line: b.0,
                column: b.1,
            }),
        });
        self.content
            .perform(text_editor::Action::Edit(text_editor::Edit::Backspace));
    }

    /// Replace an exact byte span as one undoable edit.
    fn replace_span(&mut self, start: text::Pos, end: text::Pos, replacement: String) {
        self.history.record(self.snapshot(), false);
        self.content.move_to(text_editor::Cursor {
            position: text_editor::Position {
                line: start.0,
                column: start.1,
            },
            selection: Some(text_editor::Position {
                line: end.0,
                column: end.1,
            }),
        });
        self.content
            .perform(text_editor::Action::Edit(text_editor::Edit::Paste(
                Arc::new(replacement),
            )));
        self.modified = true;
    }

    /// Discard an active phantom by removing its remaining ghost text from the
    /// buffer — the deleted sentence stays gone.
    fn phantom_discard(&mut self) {
        if let Some(rem) = self.phantom.take() {
            let cur = self.cursor_pos();
            self.delete_span(cur, text::advance(cur, &rem));
            self.modified = true;
        }
    }

    /// Accept an active phantom: keep its text as real content, moving the
    /// cursor to its end.
    fn phantom_accept(&mut self) {
        if let Some(rem) = self.phantom.take() {
            let cur = self.cursor_pos();
            self.move_to(text::advance(cur, &rem));
            self.modified = true;
        }
    }

    /// Trim the first word off an active phantom (CTRL+BACKSPACE) — the word
    /// closest to the cursor, since the ghost sits just after it. Removes that
    /// word (and its trailing space) from the buffer and keeps the tail.
    fn phantom_trim_word(&mut self) {
        let Some(rem) = self.phantom.take() else {
            return;
        };
        let cur = self.cursor_pos();
        let end = text::first_word_end(&rem);
        // The first word sits flush after the cursor; delete it from the front,
        // which leaves the cursor exactly where it was, ahead of the tail.
        self.delete_span(cur, text::advance(cur, &rem[..end]));
        self.modified = true;
        let tail = &rem[end..];
        if !tail.trim().is_empty() {
            self.phantom = Some(tail.to_string());
        }
    }

    /// The buffer's lines as owned strings, for the `core::text` algorithms.
    pub fn lines(&self) -> Vec<String> {
        (0..self.content.line_count())
            .map(|i| {
                self.content
                    .line(i)
                    .map(|l| l.text.to_string())
                    .unwrap_or_default()
            })
            .collect()
    }

    fn save(&mut self) -> Result<PathBuf, String> {
        // A pending phantom is ghost text, not document content — drop it so it
        // never reaches disk.
        self.phantom_discard();
        let path = self
            .path
            .clone()
            .ok_or("no filename — create one in the sidebar")?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let text = self.content.text();
        let mut file_text = text.clone();
        if !file_text.ends_with('\n') {
            file_text.push('\n');
        }
        std::fs::write(&path, file_text).map_err(|e| e.to_string())?;
        // Baseline is the buffer text (without the newline we add only on disk),
        // so a freshly-saved document reads as unmodified.
        self.saved_text = text;
        self.modified = false;
        Ok(path)
    }

    /// Recompute the unsaved-changes flag by comparing the buffer to the last
    /// saved/loaded text — so making an edit and then reverting it (by hand or
    /// via undo back to the saved state) clears the marker. A pending phantom
    /// always counts as modified: it is a deletion the next save will commit.
    fn refresh_modified(&mut self) {
        self.modified = self.phantom.is_some() || self.content.text() != self.saved_text;
    }

    fn invalidate_spelling(&mut self, enabled: bool) {
        self.spell_revision = self.spell_revision.wrapping_add(1);
        self.spell_issues.clear();
        self.spell_dirty_since = (enabled && self.phantom.is_none()).then(Instant::now);
    }
}

/// One rasterized PDF page: its image plus aspect ratio (height / width), so
/// the preview can scale every page to the pane width and keep proportions.
#[derive(Debug, Clone)]
pub struct PdfPage {
    pub handle: image::Handle,
    pub aspect: f32,
}

/// The right pane's content.
pub enum Preview {
    None,
    Markdown(markdown::Content),
    /// All pages, stacked top-to-bottom in a scrollable.
    Pdf(Vec<PdfPage>),
}

/// Dialog waiting on the user when there are unsaved changes.
#[derive(Debug, Clone)]
pub enum PendingAction {
    /// Close the whole window (all documents).
    CloseWindow,
    /// Close one editor pane (and discard or save its document).
    ClosePane(pane_grid::Pane),
}

pub struct App {
    pub config: Config,
    pub theme: FlowTheme,
    /// Editor typeface, resolved from `config.editor_font`.
    pub editor_font: iced::Font,
    /// Open documents, keyed by id; each has exactly one editor pane.
    pub docs: BTreeMap<DocId, Document>,
    /// The focused document — drives the preview, status bar, and dimming.
    pub active: DocId,
    next_id: DocId,
    /// PDF preview zoom (1.0 = pages fit the pane width).
    pub pdf_zoom: f32,
    /// The currently held modifier set (tracked from keyboard events). Drives
    /// the sidebar keybind hints, the editor accent emphasis, and the PDF
    /// wheel's scroll-vs-zoom toggle (CTRL).
    pub modifiers: iced::keyboard::Modifiers,
    /// Typewriter centering: a per-frame animation is converging the active
    /// paragraph toward the viewport centre. Gates the `frames` subscription.
    centering: bool,
    /// The user scrolled the editor by hand; suspends centering until the next
    /// edit (per the agreed behavior).
    user_scrolled: bool,
    pub panes: pane_grid::State<PaneKind>,
    /// The pane that last received a click; gets the highlighted border.
    pub focused: pane_grid::Pane,
    pub sidebar: Sidebar,
    pub confirm: Option<PendingAction>,
    /// File-or-folder choice shown before CTRL+O launches a native picker.
    pub open_picker: bool,
    /// The CTRL+F find bar, when open.
    pub search: Option<Search>,
    /// The CTRL+. correction chooser, when open.
    pub spell_correction: Option<SpellCorrection>,
    /// Parsed dictionary shared by background scans and suggestion jobs.
    spell_dictionary: Option<Arc<RwLock<spellbook::Dictionary>>>,
    spell_loading: bool,
    spell_load_revision: u64,
    /// The escape menu (command bar), when open.
    pub menu: Option<Menu>,
    /// The editor/preview split, for live ratio changes from the menu.
    preview_split: Option<pane_grid::Split>,
    pub status: Option<(String, Instant)>,
    /// Last-seen modification times of the watched config files, for
    /// hot-reload (see [`App::poll_config`]).
    config_sig: Vec<Option<SystemTime>>,
}

impl App {
    pub fn boot() -> (Self, Task<Message>) {
        let (config, config_warning) = Config::load();
        let (theme, theme_warning) = config.load_theme();

        let doc = match std::env::args().nth(1) {
            Some(arg) => Document::open(PathBuf::from(arg)),
            None => Document::untitled(),
        };

        let editor_font = resolve_font(&config.editor_font);
        let first_id: DocId = 0;
        let (panes, first) = pane_grid::State::new(PaneKind::Editor(first_id));
        let mut docs = BTreeMap::new();
        docs.insert(first_id, doc);
        let mut app = Self {
            config,
            theme,
            editor_font,
            docs,
            active: first_id,
            next_id: first_id + 1,
            pdf_zoom: 1.0,
            modifiers: iced::keyboard::Modifiers::default(),
            centering: false,
            user_scrolled: false,
            panes,
            focused: first,
            sidebar: Sidebar::new(PathBuf::from(".")),
            confirm: None,
            open_picker: false,
            search: None,
            spell_correction: None,
            spell_dictionary: None,
            spell_loading: false,
            spell_load_revision: 0,
            menu: None,
            preview_split: None,
            status: None,
            config_sig: Vec::new(),
        };
        app.config_sig = app.config_signature();
        app.sync_preview_pane();
        if let Some(w) = config_warning.or(theme_warning) {
            app.set_status(w);
        }
        // Start ready to type.
        let focus = view::editor::focus(app.active);
        let spelling = app.load_spell_dictionary();
        (app, Task::batch([focus, spelling]))
    }

    /// The focused document.
    pub fn active_doc(&self) -> &Document {
        &self.docs[&self.active]
    }

    fn active_doc_mut(&mut self) -> &mut Document {
        self.docs.get_mut(&self.active).expect("active doc exists")
    }

    /// The pane currently showing document `id`, if any.
    fn pane_of_doc(&self, id: DocId) -> Option<pane_grid::Pane> {
        self.panes
            .iter()
            .find(|(_, kind)| matches!(kind, PaneKind::Editor(d) if *d == id))
            .map(|(pane, _)| *pane)
    }

    /// Number of editor panes (the preview pane doesn't count).
    pub fn editor_count(&self) -> usize {
        self.panes
            .iter()
            .filter(|(_, kind)| matches!(kind, PaneKind::Editor(_)))
            .count()
    }

    /// Paths of all open documents — for the sidebar's open-file highlight.
    pub fn open_paths(&self) -> std::collections::BTreeSet<PathBuf> {
        self.docs.values().filter_map(|d| d.path.clone()).collect()
    }

    pub fn title(&self) -> String {
        format!("{} — flow-state", self.active_doc().display_name())
    }

    pub fn theme(&self) -> Theme {
        self.theme.iced_theme()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        // ESC is caught with `listen_with` (every event, not just ignored
        // ones) because the command bar's focused text input *captures* ESC
        // to unfocus itself — `listen` would never see it, so closing the bar
        // would need a second press.
        let mut subs = vec![
            window::close_requests().map(|_| Message::CloseRequested),
            iced::event::listen_with(on_escape),
            // Track the held-modifier set: keybind hints, accent emphasis, and
            // the PDF wheel's scroll-vs-zoom toggle all read it.
            iced::event::listen_with(on_modifiers),
            // CTRL+F toggles find from anywhere (so it can also close the bar).
            iced::event::listen_with(on_find_toggle),
            // Re-focus the editor when the window gains focus, so the caret
            // shows at launch without needing a click.
            iced::event::listen_with(on_window_focused),
        ];
        if self.menu.is_some() {
            // The command bar's filter input ignores arrow keys, so they
            // arrive here and drive the list selection.
            subs.push(iced::event::listen_with(on_menu_arrows));
        }
        if self.spell_correction.is_some() {
            subs.push(iced::event::listen_with(on_spell_arrows));
        }
        // Per-frame ticks only while a centering animation is converging — no
        // idle repaint when the active paragraph is already centered.
        if self.config.typewriter_scroll && self.centering {
            subs.push(iced::window::frames().map(|_| Message::CenterTick));
        }
        if self.config.spell_check
            && self.spell_dictionary.is_some()
            && self
                .docs
                .get(&self.active)
                .is_some_and(|doc| doc.spell_dirty_since.is_some())
        {
            subs.push(iced::time::every(Duration::from_millis(100)).map(|_| Message::SpellTick));
        }
        // Always-on 1 s tick: expires status messages and polls the config
        // files for hot-reload.
        subs.push(iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick));
        Subscription::batch(subs)
    }

    pub fn view(&self) -> Element<'_, Message> {
        view::view(self)
    }

    fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    fn load_spell_dictionary(&mut self) -> Task<Message> {
        self.spell_load_revision = self.spell_load_revision.wrapping_add(1);
        let load_revision = self.spell_load_revision;
        self.spell_dictionary = None;
        self.spell_loading = false;
        self.spell_correction = None;
        for doc in self.docs.values_mut() {
            doc.spell_revision = doc.spell_revision.wrapping_add(1);
            doc.spell_issues.clear();
            doc.spell_dirty_since = None;
        }
        if !self.config.spell_check {
            return Task::none();
        }

        self.spell_loading = true;
        let language = self.config.spell_language.clone();
        let configured = self.config.spell_dictionary.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    core::spell::load_dictionary(&language, &configured)
                })
                .await
                .unwrap_or_else(|e| Err(format!("spell dictionary task failed: {e}")))
            },
            move |result| Message::SpellDictionaryLoaded(load_revision, result),
        )
    }

    fn mark_all_spelling_dirty(&mut self) {
        let enabled = self.config.spell_check && self.spell_dictionary.is_some();
        for doc in self.docs.values_mut() {
            doc.invalidate_spelling(enabled);
        }
    }

    fn start_spell_check(&mut self) -> Task<Message> {
        let Some(dictionary) = self.spell_dictionary.clone() else {
            return Task::none();
        };
        let id = self.active;
        let Some(doc) = self.docs.get_mut(&id) else {
            return Task::none();
        };
        let ready = doc
            .spell_dirty_since
            .is_some_and(|since| since.elapsed() >= SPELL_DEBOUNCE)
            && !doc.spell_in_flight
            && doc.phantom.is_none();
        if !ready {
            return Task::none();
        }

        let revision = doc.spell_revision;
        let input = doc.content.text();
        doc.spell_dirty_since = None;
        doc.spell_in_flight = true;
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    dictionary
                        .read()
                        .map(|dictionary| core::spell::check_text(&dictionary, &input))
                        .unwrap_or_default()
                })
                .await
                .unwrap_or_default()
            },
            move |issues| Message::SpellChecked(id, revision, issues),
        )
    }

    fn open_spell_correction(&mut self) -> Task<Message> {
        if !self.config.spell_check {
            self.set_status("spell checking is off — enable it from ESC");
            return Task::none();
        }
        let Some(dictionary) = self.spell_dictionary.clone() else {
            self.set_status(if self.spell_loading {
                "spell dictionary is still loading"
            } else {
                "spell dictionary unavailable"
            });
            return Task::none();
        };
        let doc = self.active_doc();
        if doc.phantom.is_some() {
            self.set_status("finish or discard the phantom before correcting spelling");
            return Task::none();
        }
        let cursor = doc.cursor_pos();
        let issue = core::spell::issue_near(&doc.spell_issues, cursor).cloned();
        if issue.is_none() {
            let Some(next) = core::spell::next_issue(&doc.spell_issues, cursor).cloned() else {
                self.set_status("no misspellings in this document");
                return Task::none();
            };
            let word = next.word.clone();
            let doc = self.active_doc_mut();
            doc.move_to(next.start);
            doc.history.break_run();
            self.set_status(format!("next misspelling: {word}"));
            self.request_center();
            return view::editor::focus(self.active);
        }
        let issue = issue.expect("checked above");
        let id = self.active;
        let revision = doc.spell_revision;
        let word = issue.word.clone();
        self.search = None;
        self.menu = None;
        self.spell_correction = Some(SpellCorrection {
            doc_id: id,
            revision,
            issue,
            input: word.clone(),
            suggestions: Vec::new(),
            selected: 0,
            loading: true,
        });
        let focus = view::spell::focus_input();
        let suggestions = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let mut suggestions = Vec::new();
                    if let Ok(dictionary) = dictionary.read() {
                        dictionary.suggest(&word, &mut suggestions);
                    }
                    suggestions.truncate(8);
                    suggestions
                })
                .await
                .unwrap_or_default()
            },
            move |suggestions| Message::SpellSuggestions(id, revision, suggestions),
        );
        Task::batch([focus, suggestions])
    }

    fn apply_spell_correction(&mut self, replacement: String) -> Task<Message> {
        let Some(correction) = self.spell_correction.as_ref() else {
            return Task::none();
        };
        let replacement = replacement.trim().to_string();
        if replacement.is_empty() {
            self.set_status("correction cannot be empty");
            return view::spell::focus_input();
        }
        if replacement == correction.issue.word {
            self.set_status("choose or type a different spelling");
            return view::spell::focus_input();
        }
        let correction = self.spell_correction.take().expect("checked above");
        let Some(doc) = self.docs.get_mut(&correction.doc_id) else {
            return Task::none();
        };
        let current_word = text::slice(&doc.lines(), correction.issue.start, correction.issue.end);
        if doc.spell_revision != correction.revision || current_word != correction.issue.word {
            self.set_status("text changed before the correction could be applied");
            return view::editor::focus(self.active);
        }

        doc.replace_span(
            correction.issue.start,
            correction.issue.end,
            replacement.clone(),
        );
        doc.refresh_modified();
        doc.invalidate_spelling(self.config.spell_check && self.spell_dictionary.is_some());
        self.set_status(format!(
            "replaced {} with {replacement}",
            correction.issue.word
        ));
        view::editor::focus(correction.doc_id)
    }

    fn add_spell_word(&mut self) -> Task<Message> {
        let Some(correction) = self.spell_correction.take() else {
            return Task::none();
        };
        let word = correction.issue.word;
        let Some(dictionary) = self.spell_dictionary.clone() else {
            self.set_status("spell dictionary unavailable");
            return view::editor::focus(self.active);
        };
        let path = match core::spell::personal_dictionary_path(&self.config.spell_language) {
            Ok(path) => path,
            Err(error) => {
                self.set_status(format!("could not add {word}: {error}"));
                return view::editor::focus(self.active);
            }
        };
        let load_revision = self.spell_load_revision;
        let saved_word = word.clone();
        let save = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let mut dictionary = dictionary
                        .write()
                        .map_err(|_| "spell dictionary lock is poisoned".to_string())?;
                    core::spell::save_personal_word(&path, &saved_word)?;
                    dictionary
                        .add(&saved_word)
                        .map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|error| Err(format!("personal dictionary task failed: {error}")))
            },
            move |result| Message::SpellWordSaved(load_revision, word, result),
        );
        Task::batch([save, view::editor::focus(self.active)])
    }

    /// Start (or continue) a typewriter-centering animation after a cursor move
    /// or edit, unless the user has scrolled by hand. A no-op when typewriter
    /// scrolling is off; the animation self-stops once centered.
    fn request_center(&mut self) {
        if self.config.typewriter_scroll && !self.user_scrolled {
            self.centering = true;
        }
    }

    /// Re-run the find for `query` over the focused document and select the
    /// first match at or after the cursor (so CTRL+F finds forward). A no-op
    /// when no find bar is open.
    fn run_search(&mut self, query: String) {
        let lines = self.active_doc().lines();
        let origin = self.search.as_ref().map_or((0, 0), |s| s.origin);
        let matches = text::find_all(&lines, &query);
        let current = (!matches.is_empty()).then(|| {
            matches
                .iter()
                .position(|&(start, _)| start >= origin)
                .unwrap_or(0)
        });
        if let Some(search) = self.search.as_mut() {
            search.query = query;
            search.matches = matches;
            search.current = current;
        }
        self.select_match();
    }

    /// Move the find selection by `dir` (+1 next, −1 previous), wrapping.
    fn step_match(&mut self, dir: isize) {
        if let Some(search) = self.search.as_mut()
            && !search.matches.is_empty()
        {
            let n = search.matches.len() as isize;
            let cur = search.current.unwrap_or(0) as isize;
            search.current = Some((cur + dir).rem_euclid(n) as usize);
        }
        self.select_match();
    }

    /// Select the current find match in the focused document (caret at its end,
    /// the match itself highlighted) and re-center on it.
    fn select_match(&mut self) {
        let span = self
            .search
            .as_ref()
            .and_then(|s| s.current.and_then(|i| s.matches.get(i).copied()));
        let Some((start, end)) = span else {
            return;
        };
        self.active_doc_mut().content.move_to(text_editor::Cursor {
            position: text_editor::Position {
                line: end.0,
                column: end.1,
            },
            selection: Some(text_editor::Position {
                line: start.0,
                column: start.1,
            }),
        });
        self.request_center();
    }

    /// Ensure a preview pane exists when the focused document is previewable.
    /// The single preview pane follows the focus, so this only *adds* one
    /// (splitting the active editor to its right); it is never auto-removed —
    /// the user closes it by hand and it reappears on the next previewable
    /// open or save.
    fn sync_preview_pane(&mut self) {
        let wants_preview = self.active_doc().kind() != FileKind::Plain;
        let has_preview = self
            .panes
            .iter()
            .any(|(_, kind)| *kind == PaneKind::Preview);

        if wants_preview
            && !has_preview
            && let Some(editor) = self.pane_of_doc(self.active)
            && let Some((_, split)) =
                self.panes
                    .split(pane_grid::Axis::Vertical, editor, PaneKind::Preview)
        {
            self.panes.resize(split, self.config.split_ratio());
            self.preview_split = Some(split);
        }
    }

    /// Open the root command bar and focus its input so typing filters
    /// commands immediately.
    fn open_command_bar(&mut self) -> Task<Message> {
        self.menu = Some(Menu::Commands(Picker::default()));
        view::menu::focus_input()
    }

    /// Drill into the view a root command selects (or, for toggles, just
    /// apply the change and close).
    fn run_command(&mut self, command: Command) -> Task<Message> {
        match command {
            Command::Theme => {
                self.menu = Some(Menu::Theme(Picker::default()));
                view::menu::focus_input()
            }
            Command::Font => {
                self.menu = Some(Menu::Font(Picker::default()));
                view::menu::focus_input()
            }
            Command::Compiler => {
                self.menu = Some(Menu::Compiler(Picker::default()));
                view::menu::focus_input()
            }
            Command::Split => {
                self.menu = Some(Menu::Split);
                Task::none()
            }
            Command::Dimming => {
                self.config.focus_dimming = !self.config.focus_dimming;
                self.save_config();
                self.menu = None;
                self.set_status(if self.config.focus_dimming {
                    "focus dimming on"
                } else {
                    "focus dimming off"
                });
                view::editor::focus(self.active)
            }
            Command::Typewriter => {
                self.config.typewriter_scroll = !self.config.typewriter_scroll;
                self.save_config();
                self.menu = None;
                self.set_status(if self.config.typewriter_scroll {
                    "typewriter scroll on"
                } else {
                    "typewriter scroll off"
                });
                self.request_center();
                view::editor::focus(self.active)
            }
            Command::Glow => {
                self.config.paragraph_glow = !self.config.paragraph_glow;
                self.save_config();
                self.menu = None;
                self.set_status(if self.config.paragraph_glow {
                    "paragraph glow on"
                } else {
                    "paragraph glow off"
                });
                view::editor::focus(self.active)
            }
            Command::Spelling => {
                self.config.spell_check = !self.config.spell_check;
                self.save_config();
                self.menu = None;
                self.set_status(if self.config.spell_check {
                    "spell checking on"
                } else {
                    "spell checking off"
                });
                let load = self.load_spell_dictionary();
                Task::batch([load, view::editor::focus(self.active)])
            }
            Command::Help => {
                self.menu = Some(Menu::Help);
                Task::none()
            }
        }
    }

    /// Persist the config, surfacing failures in the status bar. Refreshes the
    /// hot-reload signature so our own write doesn't trigger a reload.
    fn save_config(&mut self) {
        if let Err(e) = self.config.save() {
            self.set_status(format!("could not save config: {e}"));
        }
        self.config_sig = self.config_signature();
    }

    /// Files the hot-reload watches: `config.toml` and the active theme file.
    fn config_files(&self) -> Vec<PathBuf> {
        let Some(dir) = core::config::config_dir() else {
            return Vec::new();
        };
        let mut files = vec![dir.join("config.toml")];
        if !self.config.theme.is_empty() {
            files.push(
                dir.join("themes")
                    .join(format!("{}.toml", self.config.theme)),
            );
        }
        files
    }

    /// Modification times of the watched files — the change signal for
    /// hot-reload. A missing file reads as `None` (also a meaningful change:
    /// e.g. the config appearing or being deleted).
    fn config_signature(&self) -> Vec<Option<SystemTime>> {
        self.config_files()
            .iter()
            .map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
            .collect()
    }

    /// Re-read config and theme from disk when their files change on disk.
    /// Skipped while the command bar is open so it can't clobber the live
    /// theme/font preview the user is arrowing through.
    fn poll_config(&mut self) -> Task<Message> {
        if self.menu.is_some() {
            return Task::none();
        }
        let sig = self.config_signature();
        if sig == self.config_sig {
            return Task::none();
        }
        self.config_sig = sig;

        let old_spell = (
            self.config.spell_check,
            self.config.spell_language.clone(),
            self.config.spell_dictionary.clone(),
        );
        let (config, warning) = Config::load();
        self.config = config;
        self.theme = self.config.load_theme().0;
        self.editor_font = resolve_font(&self.config.editor_font);
        if let Some(split) = self.preview_split {
            self.panes.resize(split, self.config.split_ratio());
        }
        // A theme name change means a different file to watch.
        self.config_sig = self.config_signature();
        self.set_status(warning.unwrap_or_else(|| "config reloaded".to_string()));
        let new_spell = (
            self.config.spell_check,
            self.config.spell_language.clone(),
            self.config.spell_dictionary.clone(),
        );
        if old_spell != new_spell {
            self.load_spell_dictionary()
        } else {
            Task::none()
        }
    }

    /// The one place focus moves: records the focused pane and, when it is an
    /// editor, makes its document the active one (the preview/status/dimming
    /// all read `active`). Focusing the preview leaves `active` on the last
    /// editor, so the preview keeps showing it.
    fn set_focus(&mut self, pane: pane_grid::Pane) {
        self.focused = pane;
        if let Some(PaneKind::Editor(id)) = self.panes.get(pane) {
            self.active = *id;
        }
    }

    /// Re-establish the invariants after any structural change (close, drop):
    /// every document has a live editor pane, `active` is a living editor, and
    /// `focused` is a living pane.
    fn validate_panes(&mut self) {
        let live: std::collections::BTreeSet<DocId> = self
            .panes
            .iter()
            .filter_map(|(_, k)| match k {
                PaneKind::Editor(d) => Some(*d),
                _ => None,
            })
            .collect();
        // Drop documents whose editor pane is gone.
        self.docs.retain(|id, _| live.contains(id));
        if !live.contains(&self.active) {
            self.active = live.into_iter().next().expect("an editor remains");
        }
        if self.panes.get(self.focused).is_none() {
            self.focused = self
                .pane_of_doc(self.active)
                .or_else(|| self.panes.iter().next().map(|(p, _)| *p))
                .expect("a pane remains");
        }
    }

    /// Close a pane. The preview pane reopens on the next save; an editor pane
    /// drops its document. The last editor never closes — there must always be
    /// a document to edit.
    fn close_pane(&mut self, pane: pane_grid::Pane) {
        if matches!(self.panes.get(pane), Some(PaneKind::Editor(_))) && self.editor_count() <= 1 {
            return;
        }
        if let Some((kind, sibling)) = self.panes.close(pane) {
            if self.focused == pane {
                self.set_focus(sibling);
            }
            if kind == PaneKind::Preview {
                self.preview_split = None;
            }
            self.validate_panes();
            self.set_status("closed pane");
        }
    }

    /// Move focus to the next (`dir = 1`) or previous (`dir = -1`) pane,
    /// wrapping around. Focusing an editor pane hands it the keyboard so the
    /// cursor is live without a click; the preview pane just takes the border.
    fn cycle_pane(&mut self, dir: isize) -> Task<Message> {
        let order: Vec<pane_grid::Pane> = self.panes.iter().map(|(p, _)| *p).collect();
        let Some(i) = order.iter().position(|p| *p == self.focused) else {
            return Task::none();
        };
        let n = order.len() as isize;
        let next = order[(i as isize + dir).rem_euclid(n) as usize];
        self.set_focus(next);
        if matches!(self.panes.get(next), Some(PaneKind::Editor(_))) {
            return view::editor::focus(self.active);
        }
        Task::none()
    }

    /// The active document's paragraph range (inclusive lines), for dimming.
    pub fn active_paragraph(&self) -> (usize, usize) {
        let content = &self.active_doc().content;
        let cur = content.cursor().position.line;
        let blank = |i: usize| {
            content
                .line(i)
                .is_none_or(|l| l.text.chars().all(char::is_whitespace))
        };
        if blank(cur) {
            return (cur, cur);
        }
        let mut start = cur;
        while start > 0 && !blank(start - 1) {
            start -= 1;
        }
        let mut end = cur;
        while end + 1 < content.line_count() && !blank(end + 1) {
            end += 1;
        }
        (start, end)
    }

    /// Open `path` in a new editor pane (or focus it if already open),
    /// splitting the active editor so the current panes stay put.
    fn open_file(&mut self, path: PathBuf) -> Task<Message> {
        // Already open? Just focus it.
        if let Some(id) = self
            .docs
            .iter()
            .find(|(_, d)| d.path.as_ref() == Some(&path))
            .map(|(id, _)| *id)
        {
            if let Some(pane) = self.pane_of_doc(id) {
                self.set_focus(pane);
            } else {
                self.active = id;
            }
            return view::editor::focus(self.active);
        }

        let mut doc = Document::open(path);
        doc.invalidate_spelling(self.config.spell_check && self.spell_dictionary.is_some());
        let name = doc.display_name();

        // Reuse the active pane if it holds a pristine scratch buffer (a fresh
        // [untitled] with no edits) — opening into it loses nothing and avoids
        // leaving an empty pane behind.
        if self.active_doc().is_pristine() {
            *self.active_doc_mut() = doc;
            self.sync_preview_pane();
            self.set_status(format!("opened {name}"));
            return view::editor::focus(self.active);
        }

        let id = self.next_id;
        self.next_id += 1;
        self.docs.insert(id, doc);
        self.spawn_editor(id);
        self.set_status(format!("opened {name}"));
        view::editor::focus(self.active)
    }

    fn change_directory(&mut self, path: PathBuf) {
        match self.sidebar.set_root(path) {
            Ok(()) => self.set_status(format!(
                "current directory: {}",
                self.sidebar.root.display()
            )),
            Err(e) => self.set_status(format!("could not open folder: {e}")),
        }
    }

    fn begin_sidebar_context(&mut self, mode: ContextMode) -> Task<Message> {
        let Some(context) = self.sidebar.context.as_mut() else {
            return Task::none();
        };
        context.mode = mode;
        context.input = if mode == ContextMode::Rename {
            context
                .target
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            String::new()
        };
        sidebar::focus_context_input()
    }

    fn submit_sidebar_context(&mut self) -> Task<Message> {
        let Some(context) = self.sidebar.context.as_ref() else {
            return Task::none();
        };
        let mode = context.mode;
        let target = context.target.clone();
        let target_is_dir = context.is_dir;
        let input = context.input.clone();

        let result = match mode {
            ContextMode::CreateFile | ContextMode::CreateFolder => {
                let parent = if target_is_dir {
                    target.as_path()
                } else {
                    target.parent().unwrap_or(&self.sidebar.root)
                };
                sidebar::child_path(parent, &input).and_then(|path| {
                    if mode == ContextMode::CreateFile {
                        std::fs::OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(&path)
                            .map(|_| path)
                            .map_err(|e| e.to_string())
                    } else {
                        std::fs::create_dir(&path)
                            .map(|_| path)
                            .map_err(|e| e.to_string())
                    }
                })
            }
            ContextMode::Rename => self.rename_sidebar_target(&target, &input),
            ContextMode::Menu | ContextMode::ConfirmDelete => return Task::none(),
        };

        match result {
            Ok(path) => {
                self.sidebar.close_context();
                self.sidebar.rebuild();
                if mode == ContextMode::CreateFile {
                    self.set_status(format!("created {}", path.display()));
                    self.open_file(path)
                } else {
                    self.set_status(if mode == ContextMode::CreateFolder {
                        format!("created folder {}", path.display())
                    } else {
                        format!("renamed to {}", path.display())
                    });
                    view::editor::focus(self.active)
                }
            }
            Err(e) => {
                self.set_status(format!("filesystem action failed: {e}"));
                sidebar::focus_context_input()
            }
        }
    }

    fn rename_sidebar_target(
        &mut self,
        target: &std::path::Path,
        name: &str,
    ) -> Result<PathBuf, String> {
        let source = std::fs::canonicalize(target).map_err(|e| e.to_string())?;
        let parent = source.parent().ok_or("cannot rename this path")?;
        let destination = sidebar::child_path(parent, name)?;
        if destination == source {
            return Ok(destination);
        }
        if destination.exists() {
            return Err(format!("{} already exists", destination.display()));
        }

        let remapped: Vec<(DocId, PathBuf)> = self
            .docs
            .iter()
            .filter_map(|(id, doc)| {
                let path = doc.path.as_deref()?;
                let canonical = std::fs::canonicalize(path).ok()?;
                sidebar::remap_descendant(&canonical, &source, &destination)
                    .map(|new_path| (*id, new_path))
            })
            .collect();

        std::fs::rename(&source, &destination).map_err(|e| e.to_string())?;
        for (id, path) in remapped {
            if let Some(doc) = self.docs.get_mut(&id) {
                doc.path = Some(path);
            }
        }
        Ok(destination)
    }

    fn delete_sidebar_target(&mut self) {
        let Some(context) = self.sidebar.context.as_ref() else {
            return;
        };
        let target = context.target.clone();
        let is_dir = context.is_dir;
        let canonical = match std::fs::canonicalize(&target) {
            Ok(path) => path,
            Err(e) => {
                self.set_status(format!("delete failed: {e}"));
                return;
            }
        };
        let contains_open_document = self.docs.values().any(|doc| {
            doc.path
                .as_deref()
                .and_then(|path| std::fs::canonicalize(path).ok())
                .is_some_and(|path| path.strip_prefix(&canonical).is_ok())
        });
        if contains_open_document {
            self.set_status("close files inside this path before deleting it");
            return;
        }

        let result = if is_dir {
            std::fs::remove_dir_all(&canonical)
        } else {
            std::fs::remove_file(&canonical)
        };
        match result {
            Ok(()) => {
                self.sidebar.close_context();
                self.sidebar.rebuild();
                self.set_status(format!("deleted {}", canonical.display()));
            }
            Err(e) => self.set_status(format!("delete failed: {e}")),
        }
    }

    /// Put the already-inserted document `id` in a new editor pane (splitting
    /// the active editor, stacking vertically), make it active, and add a
    /// preview pane if it wants one.
    fn spawn_editor(&mut self, id: DocId) {
        let anchor = self.pane_of_doc(self.active).unwrap_or(self.focused);
        match self
            .panes
            .split(pane_grid::Axis::Horizontal, anchor, PaneKind::Editor(id))
        {
            Some((new_pane, _)) => self.set_focus(new_pane),
            // Split shouldn't fail, but keep `active` valid if it ever does.
            None => self.active = id,
        }
        self.sync_preview_pane();
    }

    fn save(&mut self) -> Task<Message> {
        match self.active_doc_mut().save() {
            Ok(path) => {
                self.set_status(format!("saved {}", path.display()));
                self.sidebar.rebuild();
                // Re-open the preview pane if the user closed it.
                self.sync_preview_pane();
                self.refresh_preview()
            }
            Err(e) => {
                self.set_status(format!("save failed: {e}"));
                Task::none()
            }
        }
    }

    /// Re-render (markdown) or re-compile (LaTeX) the active document's
    /// preview after a save.
    fn refresh_preview(&mut self) -> Task<Message> {
        let id = self.active;
        match self.active_doc().kind() {
            FileKind::Plain => Task::none(),
            FileKind::Markdown => {
                let text = self.active_doc().content.text();
                self.active_doc_mut().preview = Preview::Markdown(markdown::Content::parse(&text));
                Task::none()
            }
            FileKind::Latex => {
                if self.active_doc().compiling {
                    self.set_status("compile already running…");
                    return Task::none();
                }
                self.active_doc_mut().compiling = true;
                let compiler = self.config.latex_compiler.clone();
                let path = self.active_doc().path.clone().unwrap();
                Task::perform(
                    async move {
                        let result: Result<Vec<PdfPage>, String> =
                            tokio::task::spawn_blocking(move || {
                                core::latex::compile(&compiler, &path)
                                    .map(|pages| pages.into_iter().map(to_page).collect::<Vec<_>>())
                            })
                            .await
                            .unwrap_or_else(|e| Err(e.to_string()));
                        result
                    },
                    move |result| Message::Compiled(id, result),
                )
            }
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        // After any message that can change the buffer, re-derive the active
        // document's unsaved marker from a comparison with the saved text, so
        // reverting edits clears it (see `Document::refresh_modified`). Bare
        // cursor moves / scrolls / clicks don't change text, so they skip the
        // (whole-buffer) comparison.
        let changed_doc = match &message {
            Message::Edit(id, action)
                if matches!(action, text_editor::Action::Edit(_))
                    || self.docs.get(id).is_some_and(|doc| doc.phantom.is_some()) =>
            {
                Some(*id)
            }
            Message::Undo
            | Message::Redo
            | Message::DeleteSentence
            | Message::DeleteWord
            | Message::PhantomAccept
            | Message::Save => Some(self.active),
            _ => None,
        };
        let task = self.update_inner(message);
        if let Some(id) = changed_doc {
            let enabled = self.config.spell_check && self.spell_dictionary.is_some();
            if let Some(doc) = self.docs.get_mut(&id) {
                doc.refresh_modified();
                doc.invalidate_spelling(enabled);
            }
        }
        task
    }

    fn update_inner(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Edit(id, action) => {
                // An edit/click in a pane makes its document the focused one.
                if let Some(pane) = self.pane_of_doc(id) {
                    self.set_focus(pane);
                }
                // A manual wheel scroll suspends centering until the next edit;
                // apply it directly (it neither edits text nor touches the
                // phantom/undo history).
                if matches!(action, text_editor::Action::Scroll { .. }) {
                    self.user_scrolled = true;
                    self.centering = false;
                    if let Some(doc) = self.docs.get_mut(&id) {
                        doc.content.perform(action);
                    }
                    return Task::none();
                }
                // Any other edit/move/click resumes centering on the active
                // paragraph (the animation self-stops once centered).
                self.user_scrolled = false;
                self.request_center();
                let Some(doc) = self.docs.get_mut(&id) else {
                    return Task::none();
                };
                // A phantom intercepts editing: matching keystrokes fill the
                // ghost in, others push it along, and anything else abandons it.
                if doc.phantom.is_some() {
                    match &action {
                        text_editor::Action::Edit(text_editor::Edit::Insert(c)) => {
                            let c = *c;
                            let rem = doc.phantom.as_deref().unwrap_or_default();
                            if rem.starts_with(c) {
                                // Match: step over the ghost char (it stays in
                                // the buffer, now solid) without inserting.
                                let rest = rem[c.len_utf8()..].to_string();
                                doc.content
                                    .perform(text_editor::Action::Move(text_editor::Motion::Right));
                                doc.phantom = (!rest.is_empty()).then_some(rest);
                                doc.modified = true;
                            } else {
                                // Mismatch: insert normally; the ghost is pushed
                                // to the right of the new character.
                                doc.history.record(doc.snapshot(), !c.is_whitespace());
                                doc.modified = true;
                                doc.content.perform(action);
                            }
                            return Task::none();
                        }
                        text_editor::Action::Edit(text_editor::Edit::Backspace) => {
                            // Plain BACKSPACE leaves the phantom alone — only
                            // SHIFT+BACKSPACE discards it. Delete the character
                            // before the cursor as usual; the ghost stays put
                            // just after the (now moved-back) cursor.
                            doc.history.record(doc.snapshot(), false);
                            doc.modified = true;
                            doc.content.perform(action);
                            return Task::none();
                        }
                        text_editor::Action::Edit(_) | text_editor::Action::Move(_) => {
                            // Other edits/moves abandon the ghost (sentence stays
                            // deleted), then apply normally.
                            doc.history.break_run();
                            doc.phantom_discard();
                            doc.content.perform(action);
                            return Task::none();
                        }
                        // Clicks/drags/selection: abandon and let normal
                        // handling below run.
                        _ => doc.phantom_discard(),
                    }
                }
                match &action {
                    text_editor::Action::Edit(edit) => {
                        let coalesce = matches!(
                            edit,
                            text_editor::Edit::Insert(c) if !c.is_whitespace()
                        );
                        doc.history.record(doc.snapshot(), coalesce);
                        doc.modified = true;
                    }
                    text_editor::Action::Move(_)
                    | text_editor::Action::Select(_)
                    | text_editor::Action::Click(_)
                    | text_editor::Action::Drag(_) => doc.history.break_run(),
                    _ => {}
                }
                doc.content.perform(action);
                Task::none()
            }
            Message::Save => self.save(),
            Message::Undo => {
                let current = self.active_doc().snapshot();
                let restored = self.active_doc_mut().history.undo(current);
                match restored {
                    Some(s) => {
                        self.active_doc_mut().restore(s);
                        self.set_status("undo");
                    }
                    None => self.set_status("nothing to undo"),
                }
                self.request_center();
                Task::none()
            }
            Message::Redo => {
                let current = self.active_doc().snapshot();
                let restored = self.active_doc_mut().history.redo(current);
                match restored {
                    Some(s) => {
                        self.active_doc_mut().restore(s);
                        self.set_status("redo");
                    }
                    None => self.set_status("nothing to redo"),
                }
                self.request_center();
                Task::none()
            }
            Message::DeleteSentence => {
                let doc = self.active_doc_mut();
                // A second SHIFT+BACKSPACE discards an existing phantom.
                if doc.phantom.is_some() {
                    doc.history.record(doc.snapshot(), false);
                    doc.phantom_discard();
                    return Task::none();
                }
                // Otherwise "delete" the current sentence into a phantom: the
                // text stays in the buffer as dimmed ghost just after the
                // cursor, ready to be typed back, accepted, or discarded.
                let lines = doc.lines();
                let cursor = doc.cursor_pos();
                if let Some(start) = text::sentence_start_before(&lines, cursor)
                    && start != cursor
                {
                    doc.history.record(doc.snapshot(), false);
                    doc.phantom = Some(text::slice(&lines, start, cursor));
                    doc.move_to(start);
                    doc.modified = true;
                }
                self.request_center();
                Task::none()
            }
            Message::DeleteWord => {
                let doc = self.active_doc_mut();
                if doc.phantom.is_some() {
                    // Trim the last word off the phantom.
                    doc.history.record(doc.snapshot(), false);
                    doc.phantom_trim_word();
                } else {
                    // Delete the previous word (widget word semantics).
                    doc.history.record(doc.snapshot(), false);
                    doc.modified = true;
                    doc.content
                        .perform(text_editor::Action::Select(text_editor::Motion::WordLeft));
                    doc.content
                        .perform(text_editor::Action::Edit(text_editor::Edit::Backspace));
                }
                self.request_center();
                Task::none()
            }
            Message::PhantomAccept => {
                let doc = self.active_doc_mut();
                if doc.phantom.is_some() {
                    doc.phantom_accept();
                } else {
                    // No phantom: behave like a normal Tab — insert a tab.
                    doc.history.record(doc.snapshot(), false);
                    doc.modified = true;
                    doc.content
                        .perform(text_editor::Action::Edit(text_editor::Edit::Insert('\t')));
                }
                self.request_center();
                Task::none()
            }
            Message::NextParagraph => {
                let doc = self.active_doc_mut();
                let lines = doc.lines();
                let cur = doc.content.cursor().position.line;
                if let Some(line) = text::next_paragraph_start(&lines, cur) {
                    doc.move_to((line, 0));
                    doc.history.break_run();
                }
                self.request_center();
                Task::none()
            }
            Message::PrevParagraph => {
                let doc = self.active_doc_mut();
                let lines = doc.lines();
                let cur = doc.content.cursor().position.line;
                if let Some(line) = text::prev_paragraph_start(&lines, cur) {
                    doc.move_to((line, 0));
                    doc.history.break_run();
                }
                self.request_center();
                Task::none()
            }

            Message::SpellDictionaryLoaded(load_revision, result) => {
                if load_revision != self.spell_load_revision || !self.config.spell_check {
                    return Task::none();
                }
                self.spell_loading = false;
                match result {
                    Ok(loaded) => {
                        let name = loaded
                            .aff_path
                            .file_stem()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| self.config.spell_language.clone());
                        self.spell_dictionary = Some(loaded.dictionary);
                        self.mark_all_spelling_dirty();
                        self.set_status(format!("spell checking ready ({name})"));
                    }
                    Err(error) => {
                        self.spell_dictionary = None;
                        self.set_status(format!("spell checking unavailable: {error}"));
                    }
                }
                Task::none()
            }
            Message::SpellTick => self.start_spell_check(),
            Message::SpellChecked(id, revision, issues) => {
                if let Some(doc) = self.docs.get_mut(&id) {
                    doc.spell_in_flight = false;
                    if self.config.spell_check
                        && doc.spell_revision == revision
                        && doc.phantom.is_none()
                    {
                        doc.spell_issues = issues;
                    }
                }
                Task::none()
            }
            Message::OpenSpellCorrection => self.open_spell_correction(),
            Message::SpellSuggestions(id, revision, suggestions) => {
                let revision_is_current = self
                    .docs
                    .get(&id)
                    .is_some_and(|doc| doc.spell_revision == revision);
                if revision_is_current
                    && let Some(correction) = self.spell_correction.as_mut()
                    && correction.doc_id == id
                    && correction.revision == revision
                {
                    let untouched = correction.input == correction.issue.word;
                    correction.suggestions = suggestions;
                    correction.selected = 0;
                    correction.loading = false;
                    if untouched && let Some(first) = correction.suggestions.first() {
                        correction.input = first.clone();
                    }
                }
                Task::none()
            }
            Message::SpellCorrectionInput(input) => {
                if let Some(correction) = self.spell_correction.as_mut() {
                    correction.input = input;
                }
                Task::none()
            }
            Message::SpellCorrectionPrev | Message::SpellCorrectionNext => {
                if let Some(correction) = self.spell_correction.as_mut() {
                    let len = correction.suggestions.len();
                    if len > 0 {
                        let step = if matches!(message, Message::SpellCorrectionNext) {
                            1
                        } else {
                            -1
                        };
                        correction.selected =
                            (correction.selected as isize + step).rem_euclid(len as isize) as usize;
                        correction.input = correction.suggestions[correction.selected].clone();
                    }
                }
                Task::none()
            }
            Message::SpellCorrectionSubmit => {
                let replacement = self
                    .spell_correction
                    .as_ref()
                    .map(|correction| correction.input.clone())
                    .unwrap_or_default();
                self.apply_spell_correction(replacement)
            }
            Message::SpellCorrectionApply(replacement) => self.apply_spell_correction(replacement),
            Message::SpellCorrectionIgnore => {
                if let Some(correction) = self.spell_correction.take()
                    && let Some(doc) = self.docs.get_mut(&correction.doc_id)
                    && doc.spell_revision == correction.revision
                {
                    doc.spell_issues.retain(|issue| issue != &correction.issue);
                }
                view::editor::focus(self.active)
            }
            Message::AddSpellWord => self.add_spell_word(),
            Message::SpellWordSaved(load_revision, word, result) => {
                match result {
                    Ok(()) => {
                        self.set_status(format!("added {word} to personal dictionary"));
                        if load_revision == self.spell_load_revision {
                            self.mark_all_spelling_dirty();
                        } else {
                            return self.load_spell_dictionary();
                        }
                    }
                    Err(error) => {
                        self.set_status(format!("could not add {word}: {error}"));
                    }
                }
                Task::none()
            }
            Message::CloseSpellCorrection => {
                self.spell_correction = None;
                view::editor::focus(self.active)
            }

            Message::Compiled(id, result) => {
                let Some(doc) = self.docs.get_mut(&id) else {
                    return Task::none();
                };
                doc.compiling = false;
                let ok = result.is_ok();
                match result {
                    Ok(pages) => {
                        doc.preview = Preview::Pdf(pages);
                        doc.compile_error = None;
                    }
                    Err(e) => doc.compile_error = Some(e),
                }
                self.set_status(if ok { "compiled ✓" } else { "compile failed" });
                Task::none()
            }
            Message::ModifiersChanged(m) => {
                self.modifiers = m;
                Task::none()
            }
            Message::CenterTick => {
                if !self.config.typewriter_scroll || self.user_scrolled {
                    self.centering = false;
                    return Task::none();
                }
                let para = self.active_paragraph();
                let scroll = |app: &App| {
                    app.active_doc()
                        .content
                        .with_buffer(|b| (b.scroll().line, b.scroll().vertical))
                };
                let step = self
                    .active_doc()
                    .content
                    .with_buffer(|buf| view::editor::center_step(buf, para));
                match step {
                    Some(lines) => {
                        let before = scroll(self);
                        self.active_doc_mut()
                            .content
                            .perform(text_editor::Action::Scroll { lines });
                        // Stop if the scroll clamped (top/bottom of document) —
                        // otherwise we'd spin requesting frames forever.
                        if scroll(self) == before {
                            self.centering = false;
                        }
                    }
                    None => self.centering = false,
                }
                Task::none()
            }
            Message::WindowFocused => {
                // Re-assert editor focus so the caret is visible on launch (and
                // after alt-tabbing back). Skip it while an overlay owns the
                // keyboard, or the active pane isn't an editor.
                if self.menu.is_some()
                    || self.search.is_some()
                    || self.spell_correction.is_some()
                    || self.confirm.is_some()
                    || self.open_picker
                    || self.sidebar.context_is_editing()
                {
                    return Task::none();
                }
                if matches!(self.panes.get(self.focused), Some(PaneKind::Editor(_))) {
                    return view::editor::focus(self.active);
                }
                Task::none()
            }
            Message::PdfScroll(delta) => {
                // Only reached when CTRL is held (the view only attaches the
                // scroll handler then) — zoom around the pane.
                let y = match delta {
                    iced::mouse::ScrollDelta::Lines { y, .. }
                    | iced::mouse::ScrollDelta::Pixels { y, .. } => y,
                };
                if y != 0.0 {
                    let factor = 1.0 + 0.1 * y.signum();
                    self.pdf_zoom = (self.pdf_zoom * factor).clamp(0.3, 5.0);
                }
                Task::none()
            }
            Message::DismissError => {
                self.active_doc_mut().compile_error = None;
                view::editor::focus(self.active)
            }
            Message::LinkClicked(uri) => {
                self.set_status(format!("link: {uri}"));
                Task::none()
            }

            Message::PaneDragged(pane_grid::DragEvent::Dropped { pane, target }) => {
                self.panes.drop(pane, target);
                // The dragged pane keeps focus; re-derive `active` from
                // whatever now sits there.
                self.set_focus(self.focused);
                self.validate_panes();
                Task::none()
            }
            Message::PaneDragged(_) => Task::none(),
            Message::PaneResized(pane_grid::ResizeEvent { split, ratio }) => {
                self.panes.resize(split, ratio);
                Task::none()
            }
            Message::PaneClicked(pane) => {
                self.set_focus(pane);
                Task::none()
            }
            Message::ToggleMaximize(pane) => {
                if self.panes.maximized() == Some(pane) {
                    self.panes.restore();
                } else {
                    self.panes.maximize(pane);
                }
                Task::none()
            }
            Message::ClosePane(pane) => {
                match self.panes.get(pane) {
                    Some(PaneKind::Editor(id)) => {
                        let id = *id;
                        if self.docs.get(&id).is_some_and(|d| d.modified) {
                            // Confirm before discarding unsaved changes; the
                            // dialog takes over, so don't refocus the editor.
                            self.confirm = Some(PendingAction::ClosePane(pane));
                            return Task::none();
                        }
                        self.close_pane(pane);
                    }
                    Some(PaneKind::Preview) => self.close_pane(pane),
                    None => return Task::none(),
                }
                // Closing moved focus to a sibling pane — give the editor the
                // keyboard back so the cursor is live without a click.
                view::editor::focus(self.active)
            }

            Message::EscPressed => {
                // ESC peels UI layers: dialog, error, find bar, sub-bar, bar,
                // then opens the command bar.
                if self.confirm.is_some() {
                    self.confirm = None;
                } else if self.spell_correction.is_some() {
                    self.spell_correction = None;
                    return view::editor::focus(self.active);
                } else if self.open_picker {
                    self.open_picker = false;
                    return view::editor::focus(self.active);
                } else if self.sidebar.context.is_some() {
                    self.sidebar.close_context();
                    return view::editor::focus(self.active);
                } else if self.active_doc().compile_error.is_some() {
                    self.active_doc_mut().compile_error = None;
                } else if self.search.is_some() {
                    self.search = None;
                    return view::editor::focus(self.active);
                } else {
                    match self.menu.take() {
                        // Root bar: close. Sub-views: back to the root bar.
                        Some(Menu::Commands(_)) => return view::editor::focus(self.active),
                        Some(Menu::Theme(_)) => {
                            // Cancel any live preview: back to the saved theme.
                            self.theme = self.config.load_theme().0;
                            return self.open_command_bar();
                        }
                        Some(Menu::Font(_)) => {
                            // Cancel the live font preview.
                            self.editor_font = resolve_font(&self.config.editor_font);
                            return self.open_command_bar();
                        }
                        Some(_) => return self.open_command_bar(),
                        None => return self.open_command_bar(),
                    }
                }
                Task::none()
            }
            Message::CommandSelected(command) => self.run_command(command),
            Message::CommandInput(input) => {
                match &mut self.menu {
                    Some(Menu::Commands(picker)) => {
                        // halloy-style shortcut: "?" jumps to the keybinds.
                        if input.trim() == "?" {
                            self.menu = Some(Menu::Help);
                        } else {
                            picker.input = input;
                            picker.selected = 0;
                        }
                    }
                    Some(Menu::Theme(picker)) => {
                        picker.input = input;
                        picker.selected = 0;
                        // Live preview of the top match; config stays
                        // untouched until the choice is confirmed.
                        if let Some(name) = theme_options(&picker.input).first() {
                            self.theme = load_theme_by_name(name);
                        }
                    }
                    Some(Menu::Font(picker)) => {
                        picker.input = input;
                        picker.selected = 0;
                        // Live preview of the top match.
                        if let Some(name) = font_options(&picker.input).first() {
                            self.editor_font = resolve_font(name);
                        }
                    }
                    Some(Menu::Compiler(picker)) => {
                        picker.input = input;
                        picker.selected = 0;
                    }
                    _ => {}
                }
                Task::none()
            }
            Message::MenuPrev | Message::MenuNext => {
                let step: isize = if matches!(message, Message::MenuNext) {
                    1
                } else {
                    -1
                };
                // Move the selection through the filtered list, wrapping.
                let select = |picker: &mut Picker, len: usize| {
                    if len > 0 {
                        picker.selected =
                            (picker.selected as isize + step).rem_euclid(len as isize) as usize;
                    }
                };
                match &mut self.menu {
                    Some(Menu::Commands(picker)) => {
                        let len = filtered_commands(&picker.input).len();
                        select(picker, len);
                    }
                    Some(Menu::Theme(picker)) => {
                        let options = theme_options(&picker.input);
                        select(picker, options.len());
                        if let Some(name) = options.get(picker.selected) {
                            // Live preview; config untouched until confirmed.
                            self.theme = load_theme_by_name(name);
                        }
                        return view::menu::scroll_to_selected(picker.selected);
                    }
                    Some(Menu::Font(picker)) => {
                        let options = font_options(&picker.input);
                        select(picker, options.len());
                        if let Some(name) = options.get(picker.selected) {
                            self.editor_font = resolve_font(name);
                        }
                        return view::menu::scroll_to_selected(picker.selected);
                    }
                    Some(Menu::Compiler(picker)) => {
                        let len = compiler_options(&picker.input).len();
                        select(picker, len);
                    }
                    _ => {}
                }
                Task::none()
            }
            // ENTER in the filter input: act on the selected row.
            Message::MenuSubmit => {
                let chosen = match &self.menu {
                    Some(Menu::Commands(picker)) => filtered_commands(&picker.input)
                        .get(picker.selected)
                        .copied()
                        .map(Message::CommandSelected),
                    Some(Menu::Theme(picker)) => theme_options(&picker.input)
                        .get(picker.selected)
                        .cloned()
                        .map(Message::ThemeSelected),
                    Some(Menu::Font(picker)) => font_options(&picker.input)
                        .get(picker.selected)
                        .cloned()
                        .map(Message::FontSelected),
                    Some(Menu::Compiler(picker)) => compiler_options(&picker.input)
                        .get(picker.selected)
                        .cloned()
                        .map(Message::CompilerSelected),
                    _ => None,
                };
                match chosen {
                    Some(message) => self.update(message),
                    None => Task::none(),
                }
            }
            Message::ThemeSelected(name) => {
                self.config.theme = if name == core::config::BUILTIN_THEME {
                    String::new()
                } else {
                    name
                };
                self.theme = self.config.load_theme().0;
                self.save_config();
                self.menu = None;
                view::editor::focus(self.active)
            }
            Message::CompilerSelected(compiler) => {
                self.config.latex_compiler = compiler;
                self.save_config();
                self.menu = None;
                view::editor::focus(self.active)
            }
            Message::FontSelected(name) => {
                self.config.editor_font = if name == core::config::BUILTIN_THEME {
                    String::new()
                } else {
                    name
                };
                self.editor_font = resolve_font(&self.config.editor_font);
                self.save_config();
                self.menu = None;
                view::editor::focus(self.active)
            }
            Message::SplitRatioChanged(ratio) => {
                self.config.preview_split_ratio = ratio;
                if let Some(split) = self.preview_split {
                    self.panes.resize(split, ratio);
                }
                Task::none()
            }
            // Persist once, when the slider is let go (not per drag tick).
            Message::SplitRatioReleased => {
                self.save_config();
                Task::none()
            }

            Message::ToggleSidebar => {
                self.sidebar.toggle_collapsed();
                self.sidebar.close_context();
                view::editor::focus(self.active)
            }
            Message::ToggleDir(path) => {
                self.sidebar.close_context();
                self.sidebar.toggle(path);
                Task::none()
            }
            Message::ChangeDirectory(path) => {
                self.change_directory(path);
                Task::none()
            }
            Message::OpenFile(path) => {
                self.sidebar.close_context();
                self.open_file(path)
            }
            Message::OpenSidebarContext(path, is_dir) => {
                self.sidebar.open_context(path, is_dir);
                Task::none()
            }
            Message::CloseSidebarContext => {
                self.sidebar.close_context();
                view::editor::focus(self.active)
            }
            Message::SidebarContextCreateFile => {
                self.begin_sidebar_context(ContextMode::CreateFile)
            }
            Message::SidebarContextCreateFolder => {
                self.begin_sidebar_context(ContextMode::CreateFolder)
            }
            Message::SidebarContextRename => self.begin_sidebar_context(ContextMode::Rename),
            Message::SidebarContextDelete => {
                if let Some(context) = self.sidebar.context.as_mut() {
                    context.mode = ContextMode::ConfirmDelete;
                }
                Task::none()
            }
            Message::SidebarContextConfirmDelete => {
                self.delete_sidebar_target();
                Task::none()
            }
            Message::SidebarContextInput(input) => {
                if let Some(context) = self.sidebar.context.as_mut() {
                    context.input = input;
                }
                Task::none()
            }
            Message::SidebarContextSubmit => self.submit_sidebar_context(),
            Message::NewFileInput(input) => {
                self.sidebar.new_file = input;
                Task::none()
            }
            Message::CreateFile => {
                let name = self.sidebar.new_file.trim().to_string();
                if name.is_empty() {
                    return Task::none();
                }
                self.sidebar.new_file.clear();
                let path = match sidebar::child_path(&self.sidebar.root, &name) {
                    Ok(path) => path,
                    Err(e) => {
                        self.set_status(e);
                        return Task::none();
                    }
                };
                if self.active_doc().path.is_none() {
                    // Untitled buffer: keep its contents, just give it a name.
                    let doc = self.active_doc_mut();
                    doc.path = Some(path);
                    doc.modified = true;
                    self.sync_preview_pane();
                    self.set_status(format!("created {name} — CTRL+S to save"));
                    Task::none()
                } else {
                    // Otherwise open the new (empty) file in its own pane.
                    let mut doc = Document::untitled();
                    doc.path = Some(path);
                    doc.modified = true;
                    let id = self.next_id;
                    self.next_id += 1;
                    self.docs.insert(id, doc);
                    self.spawn_editor(id);
                    self.set_status(format!("created {name} — CTRL+S to save"));
                    view::editor::focus(self.active)
                }
            }

            Message::NewFile => {
                // A fresh scratch buffer in its own pane (CTRL+N).
                let id = self.next_id;
                self.next_id += 1;
                self.docs.insert(id, Document::untitled());
                self.spawn_editor(id);
                self.set_status("new file");
                view::editor::focus(self.active)
            }
            Message::OpenFilePicker => {
                self.sidebar.close_context();
                self.open_picker = true;
                Task::none()
            }
            Message::ChooseFileToOpen => {
                self.open_picker = false;
                let root = self.sidebar.root.clone();
                Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_directory(root)
                            .pick_file()
                            .await
                            .map(|h| h.path().to_path_buf())
                    },
                    Message::FilePicked,
                )
            }
            Message::ChooseFolderToOpen => {
                self.open_picker = false;
                let root = self.sidebar.root.clone();
                Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_directory(root)
                            .pick_folder()
                            .await
                            .map(|h| h.path().to_path_buf())
                    },
                    Message::FolderPicked,
                )
            }
            Message::CloseOpenPicker => {
                self.open_picker = false;
                view::editor::focus(self.active)
            }
            Message::FilePicked(picked) => match picked {
                Some(path) => self.open_file(path),
                None => Task::none(),
            },
            Message::FolderPicked(picked) => {
                if let Some(path) = picked {
                    self.change_directory(path);
                }
                Task::none()
            }
            Message::CloseActivePane => self.update(Message::ClosePane(self.focused)),
            Message::NextPane => self.cycle_pane(1),
            Message::PrevPane => self.cycle_pane(-1),
            Message::ToggleSearch => {
                if self.search.take().is_some() {
                    // Second CTRL+F closes the bar and returns to the editor.
                    view::editor::focus(self.active)
                } else if self.menu.is_some()
                    || self.spell_correction.is_some()
                    || self.confirm.is_some()
                    || self.open_picker
                {
                    Task::none() // don't pop find over a modal
                } else {
                    let origin = self.active_doc().cursor_pos();
                    self.search = Some(Search {
                        origin,
                        ..Search::default()
                    });
                    view::search::focus_input()
                }
            }
            Message::SearchInput(query) => {
                self.run_search(query);
                Task::none()
            }
            Message::SearchNext => {
                self.step_match(1);
                Task::none()
            }
            Message::SearchPrev => {
                self.step_match(-1);
                Task::none()
            }
            Message::CloseSearch => {
                self.search = None;
                view::editor::focus(self.active)
            }

            Message::CloseRequested => {
                if self.docs.values().any(|d| d.modified) {
                    self.confirm = Some(PendingAction::CloseWindow);
                    Task::none()
                } else {
                    iced::exit()
                }
            }
            Message::ConfirmSave => {
                let Some(action) = self.confirm.take() else {
                    return Task::none();
                };
                match action {
                    PendingAction::CloseWindow => {
                        // Save every modified document, then exit.
                        let mut failed = None;
                        for doc in self.docs.values_mut().filter(|d| d.modified) {
                            if let Err(e) = doc.save() {
                                failed = Some(e);
                                break;
                            }
                        }
                        match failed {
                            None => iced::exit(),
                            Some(e) => {
                                self.set_status(format!("save failed: {e}"));
                                Task::none()
                            }
                        }
                    }
                    PendingAction::ClosePane(pane) => {
                        let id = match self.panes.get(pane) {
                            Some(PaneKind::Editor(id)) => *id,
                            _ => return Task::none(),
                        };
                        match self.docs.get_mut(&id).map(|d| d.save()) {
                            Some(Ok(_)) => {
                                self.close_pane(pane);
                                view::editor::focus(self.active)
                            }
                            Some(Err(e)) => {
                                self.set_status(format!("save failed: {e}"));
                                Task::none()
                            }
                            None => Task::none(),
                        }
                    }
                }
            }
            Message::ConfirmDiscard => {
                let Some(action) = self.confirm.take() else {
                    return Task::none();
                };
                match action {
                    PendingAction::CloseWindow => iced::exit(),
                    PendingAction::ClosePane(pane) => {
                        self.close_pane(pane);
                        view::editor::focus(self.active)
                    }
                }
            }
            Message::ConfirmCancel => {
                self.confirm = None;
                view::editor::focus(self.active)
            }
            Message::Tick => {
                if let Some((_, since)) = &self.status
                    && since.elapsed() > STATUS_TTL
                {
                    self.status = None;
                }
                self.poll_config()
            }
        }
    }
}

fn to_page(img: ::image::DynamicImage) -> PdfPage {
    let rgba = img.into_rgba8();
    let (w, h) = rgba.dimensions();
    PdfPage {
        aspect: h as f32 / w as f32,
        handle: image::Handle::from_rgba(w, h, rgba.into_raw()),
    }
}

#[cfg(test)]
mod tests {
    use super::Document;
    use crate::core::spell::SpellIssue;
    use crate::view::widget::text_editor;

    #[test]
    fn spelling_correction_is_one_undoable_edit() {
        let mut doc = Document::untitled();
        doc.content = text_editor::Content::with_text("hello wurld");
        doc.replace_span((0, 6), (0, 11), "world".to_string());
        assert_eq!(doc.content.text(), "hello world");

        let current = doc.snapshot();
        let previous = doc.history.undo(current).unwrap();
        doc.restore(previous);
        assert_eq!(doc.content.text(), "hello wurld");
    }

    #[test]
    fn spelling_invalidation_clears_stale_spans_and_waits_for_phantoms() {
        let mut doc = Document::untitled();
        doc.spell_issues.push(SpellIssue {
            start: (0, 0),
            end: (0, 5),
            word: "wurld".to_string(),
        });
        let revision = doc.spell_revision;
        doc.invalidate_spelling(true);
        assert!(doc.spell_issues.is_empty());
        assert_ne!(doc.spell_revision, revision);
        assert!(doc.spell_dirty_since.is_some());

        doc.phantom = Some("ghost".to_string());
        doc.invalidate_spelling(true);
        assert!(doc.spell_dirty_since.is_none());
    }
}
