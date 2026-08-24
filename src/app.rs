//! Application state and update logic (Elm architecture).
//!
//! [`App`] coordinates the [`Workspace`], sidebar, transient [`UiState`], and
//! external tasks. Each aggregate owns its own invariants; [`App::update`] is
//! the single event entry point and [`crate::view`] renders it. Slow work
//! (LaTeX compiles) runs off-thread via [`Task::perform`] and comes back as a
//! [`Message::Compiled`].
//!
//! Multiple files can be open at once: each lives in its own editor pane,
//! keyed by [`DocId`]. The single preview pane follows the focused editor
//! ([`App::active`]) — it renders that document's preview, status bar, and
//! paragraph dimming.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use iced::widget::{image, markdown, pane_grid};
use iced::{Element, Subscription, Task, Theme, window};

use crate::view::widget::text_editor;

use crate::core::config::Config;
use crate::core::spell::{LoadedDictionary, SpellIssue};
use crate::core::theme::Theme as FlowTheme;
use crate::core::{self, FileKind, text};
use crate::document::{Document, PdfPage, Preview};
use crate::selection::{SelectionFormat, SelectionMenu, TextSelection};
use crate::ui_state::UiState;
pub use crate::workspace::PaneKind;
use crate::workspace::{ClosePaneResult, Workspace};
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
pub enum SelectionMessage {
    Open(DocId, iced::Point, iced::Size),
    Close,
    Copy,
    Cut,
    Paste,
    PasteReady(DocId, String, Option<String>),
    SpellCorrect,
    Format(SelectionFormat),
}

#[derive(Debug, Clone)]
pub enum WorkspaceMessage {
    PaneDragged(pane_grid::DragEvent),
    PaneResized(pane_grid::ResizeEvent),
    PaneClicked(pane_grid::Pane),
    ToggleMaximize(pane_grid::Pane),
    ToggleFullscreen(pane_grid::Pane),
    ClosePane(pane_grid::Pane),
}

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
    Selection(SelectionMessage),
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
    Workspace(WorkspaceMessage),
    // sidebar
    ToggleSidebar,
    ToggleDir(PathBuf),
    ChangeDirectory(PathBuf),
    OpenFile(PathBuf),
    OpenSidebarContext(PathBuf, bool),
    CloseSidebarContext,
    SidebarContextChangeDirectory,
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
    core::config::resolve_theme(name).0
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

/// CTRL+TAB fallback while fullscreen. The editor handles the shortcut itself;
/// this catches it only when the fullscreen pane (notably preview) ignored it.
fn on_fullscreen_pane_cycle(
    event: iced::Event,
    status: iced::event::Status,
    _window: window::Id,
) -> Option<Message> {
    use iced::keyboard::{Event, Key, key::Named};
    if status != iced::event::Status::Ignored {
        return None;
    }
    match event {
        iced::Event::Keyboard(Event::KeyPressed {
            key: Key::Named(Named::Tab),
            modifiers,
            ..
        }) if modifiers.control() => Some(if modifiers.shift() {
            Message::PrevPane
        } else {
            Message::NextPane
        }),
        _ => None,
    }
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

fn toggled_fullscreen(
    current: Option<pane_grid::Pane>,
    target: pane_grid::Pane,
) -> Option<pane_grid::Pane> {
    (current != Some(target)).then_some(target)
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
    pub workspace: Workspace,
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
    pub sidebar: Sidebar,
    pub ui: UiState,
    /// Parsed dictionary shared by background scans and suggestion jobs.
    spell_dictionary: Option<Arc<RwLock<spellbook::Dictionary>>>,
    spell_loading: bool,
    spell_load_revision: u64,
    /// The editor/preview split, for live ratio changes from the menu.
    preview_split: Option<pane_grid::Split>,
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
        let mut app = Self {
            config,
            theme,
            editor_font,
            workspace: Workspace::new(first_id, doc),
            pdf_zoom: 1.0,
            modifiers: iced::keyboard::Modifiers::default(),
            centering: false,
            user_scrolled: false,
            sidebar: Sidebar::new(PathBuf::from(".")),
            ui: UiState::default(),
            spell_dictionary: None,
            spell_loading: false,
            spell_load_revision: 0,
            preview_split: None,
            config_sig: Vec::new(),
        };
        app.config_sig = app.config_signature();
        app.sync_preview_pane();
        if let Some(w) = config_warning.or(theme_warning) {
            app.set_status(w);
        }
        // Start ready to type.
        let focus = view::editor::focus(app.workspace.active_id());
        let spelling = app.load_spell_dictionary();
        (app, Task::batch([focus, spelling]))
    }

    /// The focused document.
    pub fn active_doc(&self) -> &Document {
        self.workspace.active_document()
    }

    fn active_doc_mut(&mut self) -> &mut Document {
        self.workspace.active_document_mut()
    }

    pub fn selection_is_markdown(&self) -> bool {
        self.ui.selection_menu.as_ref().is_some_and(|menu| {
            self.workspace.documents
                .get(&menu.target.document())
                .is_some_and(|doc| doc.kind() == FileKind::Markdown)
        })
    }

    pub fn selection_can_spell_correct(&self) -> bool {
        let Some(menu) = &self.ui.selection_menu else {
            return false;
        };
        let Some(doc) = self.workspace.document(menu.target.document()) else {
            return false;
        };
        self.config.spell_check
            && self.spell_dictionary.is_some()
            && doc
                .spell_issues()
                .iter()
                .any(|issue| issue.start == menu.target.start() && issue.end == menu.target.end())
    }

    fn current_selection(&self) -> Option<(DocId, String)> {
        let menu = self.ui.selection_menu.as_ref()?;
        let doc = self.workspace.document(menu.target.document())?;
        menu.target
            .is_current(doc)
            .then_some((menu.target.document(), menu.target.text().to_string()))
    }

    fn restore_selection_target(&mut self) -> Option<DocId> {
        let menu = self.ui.selection_menu.as_ref()?.clone();
        self.current_selection()?;
        let doc = self.workspace.document_mut(menu.target.document())?;
        menu.target.restore(doc).then_some(menu.target.document())
    }

    /// The pane currently showing document `id`, if any.
    fn pane_of_doc(&self, id: DocId) -> Option<pane_grid::Pane> {
        self.workspace.pane_of_document(id)
    }

    /// Number of editor panes (the preview pane doesn't count).
    pub fn editor_count(&self) -> usize {
        self.workspace.editor_count()
    }

    /// Paths of all open documents — for the sidebar's open-file highlight.
    pub fn open_paths(&self) -> std::collections::BTreeSet<PathBuf> {
        self.workspace.documents.values().filter_map(|d| d.path.clone()).collect()
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
        if self.ui.menu.is_some() {
            // The command bar's filter input ignores arrow keys, so they
            // arrive here and drive the list selection.
            subs.push(iced::event::listen_with(on_menu_arrows));
        }
        if self.ui.spell_correction.is_some() {
            subs.push(iced::event::listen_with(on_spell_arrows));
        }
        if self.workspace.fullscreen.is_some() {
            subs.push(iced::event::listen_with(on_fullscreen_pane_cycle));
        }
        // Per-frame ticks only while a centering animation is converging — no
        // idle repaint when the active paragraph is already centered.
        if self.config.typewriter_scroll && self.centering {
            subs.push(iced::window::frames().map(|_| Message::CenterTick));
        }
        if self.config.spell_check
            && self.spell_dictionary.is_some()
            && self
                .workspace.documents
                .get(&self.workspace.active)
                .is_some_and(Document::spelling_pending)
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
        self.ui.set_status(msg);
    }

    fn load_spell_dictionary(&mut self) -> Task<Message> {
        self.spell_load_revision = self.spell_load_revision.wrapping_add(1);
        let load_revision = self.spell_load_revision;
        self.spell_dictionary = None;
        self.spell_loading = false;
        self.ui.spell_correction = None;
        for doc in self.workspace.documents.values_mut() {
            doc.clear_spelling();
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
        for doc in self.workspace.documents.values_mut() {
            doc.invalidate_spelling(enabled);
        }
    }

    fn start_spell_check(&mut self) -> Task<Message> {
        let Some(dictionary) = self.spell_dictionary.clone() else {
            return Task::none();
        };
        let id = self.workspace.active;
        let Some(doc) = self.workspace.documents.get_mut(&id) else {
            return Task::none();
        };
        let Some(revision) = doc.begin_spell_scan(SPELL_DEBOUNCE) else {
            return Task::none();
        };

        let input = doc.content.text();
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
        let issue = core::spell::issue_near(doc.spell_issues(), cursor).cloned();
        if issue.is_none() {
            let Some(next) = core::spell::next_issue(doc.spell_issues(), cursor).cloned() else {
                self.set_status("no misspellings in this document");
                return Task::none();
            };
            let word = next.word.clone();
            let doc = self.active_doc_mut();
            doc.move_to(next.start);
            doc.history.break_run();
            self.set_status(format!("next misspelling: {word}"));
            self.request_center();
            return view::editor::focus(self.workspace.active);
        }
        let issue = issue.expect("checked above");
        let id = self.workspace.active;
        let revision = doc.spell_revision();
        let word = issue.word.clone();
        self.ui.search = None;
        self.ui.menu = None;
        self.ui.spell_correction = Some(SpellCorrection {
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
        let Some(correction) = self.ui.spell_correction.as_ref() else {
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
        let correction = self.ui.spell_correction.take().expect("checked above");
        let Some(doc) = self.workspace.documents.get_mut(&correction.doc_id) else {
            return Task::none();
        };
        let current_word = text::slice(&doc.lines(), correction.issue.start, correction.issue.end);
        if doc.spell_revision() != correction.revision || current_word != correction.issue.word {
            self.set_status("text changed before the correction could be applied");
            return view::editor::focus(self.workspace.active);
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
        let Some(correction) = self.ui.spell_correction.take() else {
            return Task::none();
        };
        let word = correction.issue.word;
        let Some(dictionary) = self.spell_dictionary.clone() else {
            self.set_status("spell dictionary unavailable");
            return view::editor::focus(self.workspace.active);
        };
        let path = match core::spell::personal_dictionary_path(&self.config.spell_language) {
            Ok(path) => path,
            Err(error) => {
                self.set_status(format!("could not add {word}: {error}"));
                return view::editor::focus(self.workspace.active);
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
        Task::batch([save, view::editor::focus(self.workspace.active)])
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
        let origin = self.ui.search.as_ref().map_or((0, 0), |s| s.origin);
        let matches = text::find_all(&lines, &query);
        let current = (!matches.is_empty()).then(|| {
            matches
                .iter()
                .position(|&(start, _)| start >= origin)
                .unwrap_or(0)
        });
        if let Some(search) = self.ui.search.as_mut() {
            search.query = query;
            search.matches = matches;
            search.current = current;
        }
        self.select_match();
    }

    /// Move the find selection by `dir` (+1 next, −1 previous), wrapping.
    fn step_match(&mut self, dir: isize) {
        if let Some(search) = self.ui.search.as_mut()
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
            .ui.search
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
            .workspace.panes
            .iter()
            .any(|(_, kind)| *kind == PaneKind::Preview);

        if wants_preview
            && !has_preview
            && let Some(editor) = self.pane_of_doc(self.workspace.active)
            && let Some((_, split)) =
                self.workspace.panes
                    .split(pane_grid::Axis::Vertical, editor, PaneKind::Preview)
        {
            self.workspace.panes.resize(split, self.config.split_ratio());
            self.preview_split = Some(split);
        }
    }

    /// Open the root command bar and focus its input so typing filters
    /// commands immediately.
    fn open_command_bar(&mut self) -> Task<Message> {
        self.ui.menu = Some(Menu::Commands(Picker::default()));
        view::menu::focus_input()
    }

    /// Drill into the view a root command selects (or, for toggles, just
    /// apply the change and close).
    fn run_command(&mut self, command: Command) -> Task<Message> {
        match command {
            Command::Theme => {
                self.ui.menu = Some(Menu::Theme(Picker::default()));
                view::menu::focus_input()
            }
            Command::Font => {
                self.ui.menu = Some(Menu::Font(Picker::default()));
                view::menu::focus_input()
            }
            Command::Compiler => {
                self.ui.menu = Some(Menu::Compiler(Picker::default()));
                view::menu::focus_input()
            }
            Command::Split => {
                self.ui.menu = Some(Menu::Split);
                Task::none()
            }
            Command::Dimming => {
                self.config.focus_dimming = !self.config.focus_dimming;
                self.save_config();
                self.ui.menu = None;
                self.set_status(if self.config.focus_dimming {
                    "focus dimming on"
                } else {
                    "focus dimming off"
                });
                view::editor::focus(self.workspace.active)
            }
            Command::Typewriter => {
                self.config.typewriter_scroll = !self.config.typewriter_scroll;
                self.save_config();
                self.ui.menu = None;
                self.set_status(if self.config.typewriter_scroll {
                    "typewriter scroll on"
                } else {
                    "typewriter scroll off"
                });
                self.request_center();
                view::editor::focus(self.workspace.active)
            }
            Command::Glow => {
                self.config.paragraph_glow = !self.config.paragraph_glow;
                self.save_config();
                self.ui.menu = None;
                self.set_status(if self.config.paragraph_glow {
                    "paragraph glow on"
                } else {
                    "paragraph glow off"
                });
                view::editor::focus(self.workspace.active)
            }
            Command::Spelling => {
                self.config.spell_check = !self.config.spell_check;
                self.save_config();
                self.ui.menu = None;
                self.set_status(if self.config.spell_check {
                    "spell checking on"
                } else {
                    "spell checking off"
                });
                let load = self.load_spell_dictionary();
                Task::batch([load, view::editor::focus(self.workspace.active)])
            }
            Command::Help => {
                self.ui.menu = Some(Menu::Help);
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
        if self.ui.menu.is_some() {
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
            self.workspace.panes.resize(split, self.config.split_ratio());
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
        self.workspace.focus(pane);
    }

    /// Re-establish the invariants after any structural change (close, drop):
    /// every document has a live editor pane, `active` is a living editor, and
    /// `focused` is a living pane.
    fn validate_panes(&mut self) {
        self.workspace.validate();
    }

    /// Close a pane. The preview pane reopens on the next save; an editor pane
    /// drops its document. The last editor never closes — there must always be
    /// a document to edit.
    fn close_pane(&mut self, pane: pane_grid::Pane) {
        if let ClosePaneResult::Closed { preview } = self.workspace.close_pane(pane) {
            if preview {
                self.preview_split = None;
            }
            self.set_status("closed pane");
        }
    }

    /// Move focus to the next (`dir = 1`) or previous (`dir = -1`) pane,
    /// wrapping around. Focusing an editor pane hands it the keyboard so the
    /// cursor is live without a click; the preview pane just takes the border.
    fn cycle_pane(&mut self, dir: isize) -> Task<Message> {
        if matches!(self.workspace.cycle_focus(dir), Some(PaneKind::Editor(_))) {
            return view::editor::focus(self.workspace.active);
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
            .workspace.documents
            .iter()
            .find(|(_, d)| d.path.as_ref() == Some(&path))
            .map(|(id, _)| *id)
        {
            if let Some(pane) = self.pane_of_doc(id) {
                self.set_focus(pane);
            } else {
                self.workspace.active = id;
            }
            return view::editor::focus(self.workspace.active);
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
            return view::editor::focus(self.workspace.active);
        }

        let id = self.workspace.insert_document(doc);
        self.spawn_editor(id);
        self.set_status(format!("opened {name}"));
        view::editor::focus(self.workspace.active)
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
                    view::editor::focus(self.workspace.active)
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
            .workspace.documents
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
            if let Some(doc) = self.workspace.documents.get_mut(&id) {
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
        let contains_open_document = self.workspace.documents.values().any(|doc| {
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
        self.workspace.split_editor(id);
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
        let id = self.workspace.active;
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
                    || self.workspace.documents.get(id).is_some_and(|doc| doc.phantom.is_some()) =>
            {
                Some(*id)
            }
            Message::Undo
            | Message::Redo
            | Message::DeleteSentence
            | Message::DeleteWord
            | Message::PhantomAccept
            | Message::Save => Some(self.workspace.active),
            _ => None,
        };
        let task = self.update_inner(message);
        if let Some(id) = changed_doc {
            let enabled = self.config.spell_check && self.spell_dictionary.is_some();
            if let Some(doc) = self.workspace.documents.get_mut(&id) {
                doc.refresh_modified();
                doc.invalidate_spelling(enabled);
            }
        }
        task
    }

    fn update_inner(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Edit(id, action) => {
                self.ui.selection_menu = None;
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
                    if let Some(doc) = self.workspace.documents.get_mut(&id) {
                        doc.content.perform(action);
                    }
                    return Task::none();
                }
                // Any other edit/move/click resumes centering on the active
                // paragraph (the animation self-stops once centered).
                self.user_scrolled = false;
                self.request_center();
                let Some(doc) = self.workspace.documents.get_mut(&id) else {
                    return Task::none();
                };
                doc.apply_action(action);
                Task::none()
            }
            Message::Selection(message) => match message {
            SelectionMessage::Open(id, position, bounds) => {
                let Some(target) = self
                    .workspace.documents
                    .get(&id)
                    .and_then(|doc| TextSelection::capture(id, doc))
                else {
                    self.ui.selection_menu = None;
                    return Task::none();
                };
                if let Some(pane) = self.pane_of_doc(id) {
                    self.set_focus(pane);
                }
                self.sidebar.close_context();
                let width = 196.0_f32.min((bounds.width - 16.0).max(0.0));
                let height = 278.0_f32.min((bounds.height - 16.0).max(0.0));
                let position = iced::Point::new(
                    (position.x + 6.0).clamp(8.0, (bounds.width - width - 8.0).max(8.0)),
                    (position.y + 6.0).clamp(8.0, (bounds.height - height - 8.0).max(8.0)),
                );
                self.ui.open_selection_menu(SelectionMenu { target, position });
                Task::none()
            }
            SelectionMessage::Close => {
                self.ui.selection_menu = None;
                view::editor::focus(self.workspace.active)
            }
            SelectionMessage::Copy => {
                let Some((id, selected)) = self.current_selection() else {
                    self.ui.selection_menu = None;
                    return Task::none();
                };
                self.workspace.active = id;
                self.ui.selection_menu = None;
                Task::batch([iced::clipboard::write(selected), view::editor::focus(id)])
            }
            SelectionMessage::Cut => {
                let Some((id, selected)) = self.current_selection() else {
                    self.ui.selection_menu = None;
                    return Task::none();
                };
                self.restore_selection_target();
                self.workspace.active = id;
                self.ui.selection_menu = None;
                Task::batch([
                    iced::clipboard::write(selected),
                    self.update(Message::Edit(
                        id,
                        text_editor::Action::Edit(text_editor::Edit::Delete),
                    )),
                    view::editor::focus(id),
                ])
            }
            SelectionMessage::Paste => {
                let Some((id, selected)) = self.current_selection() else {
                    self.ui.selection_menu = None;
                    return Task::none();
                };
                self.restore_selection_target();
                self.workspace.active = id;
                self.ui.selection_menu = None;
                iced::clipboard::read().map(move |contents| {
                    Message::Selection(SelectionMessage::PasteReady(
                        id,
                        selected.clone(),
                        contents,
                    ))
                })
            }
            SelectionMessage::PasteReady(id, selected, contents) => {
                if self
                    .workspace.documents
                    .get(&id)
                    .and_then(|doc| doc.content.selection())
                    .as_deref()
                    != Some(selected.as_str())
                {
                    return Task::none();
                }
                let Some(contents) = contents else {
                    self.set_status("clipboard is empty");
                    return view::editor::focus(self.workspace.active);
                };
                self.update(Message::Edit(
                    id,
                    text_editor::Action::Edit(text_editor::Edit::Paste(Arc::new(contents))),
                ))
            }
            SelectionMessage::SpellCorrect => {
                if !self.selection_can_spell_correct() {
                    return Task::none();
                }
                let id = self
                    .ui.selection_menu
                    .as_ref()
                    .expect("checked above")
                    .target
                    .document();
                self.restore_selection_target();
                self.workspace.active = id;
                self.ui.selection_menu = None;
                self.open_spell_correction()
            }
            SelectionMessage::Format(format) => {
                let Some((id, selected)) = self.current_selection() else {
                    self.ui.selection_menu = None;
                    return Task::none();
                };
                if self
                    .workspace.documents
                    .get(&id)
                    .is_none_or(|doc| doc.kind() != FileKind::Markdown)
                {
                    return Task::none();
                }
                self.restore_selection_target();
                let replacement = format.apply(&selected);
                self.workspace.active = id;
                self.ui.selection_menu = None;
                self.update(Message::Edit(
                    id,
                    text_editor::Action::Edit(text_editor::Edit::Paste(Arc::new(replacement))),
                ))
            }
            },
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
                if let Some(doc) = self.workspace.documents.get_mut(&id) {
                    doc.complete_spell_scan(revision, self.config.spell_check, issues);
                }
                Task::none()
            }
            Message::OpenSpellCorrection => self.open_spell_correction(),
            Message::SpellSuggestions(id, revision, suggestions) => {
                let revision_is_current = self
                    .workspace.documents
                    .get(&id)
                    .is_some_and(|doc| doc.spell_revision() == revision);
                if revision_is_current
                    && let Some(correction) = self.ui.spell_correction.as_mut()
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
                if let Some(correction) = self.ui.spell_correction.as_mut() {
                    correction.input = input;
                }
                Task::none()
            }
            Message::SpellCorrectionPrev | Message::SpellCorrectionNext => {
                if let Some(correction) = self.ui.spell_correction.as_mut() {
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
                    .ui.spell_correction
                    .as_ref()
                    .map(|correction| correction.input.clone())
                    .unwrap_or_default();
                self.apply_spell_correction(replacement)
            }
            Message::SpellCorrectionApply(replacement) => self.apply_spell_correction(replacement),
            Message::SpellCorrectionIgnore => {
                if let Some(correction) = self.ui.spell_correction.take()
                    && let Some(doc) = self.workspace.documents.get_mut(&correction.doc_id)
                    && doc.spell_revision() == correction.revision
                {
                    doc.ignore_spell_issue(&correction.issue);
                }
                view::editor::focus(self.workspace.active)
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
                self.ui.spell_correction = None;
                view::editor::focus(self.workspace.active)
            }

            Message::Compiled(id, result) => {
                let Some(doc) = self.workspace.documents.get_mut(&id) else {
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
                if self.ui.menu.is_some()
                    || self.ui.search.is_some()
                    || self.ui.spell_correction.is_some()
                    || self.ui.confirm.is_some()
                    || self.ui.open_picker
                    || self.sidebar.context_is_editing()
                {
                    return Task::none();
                }
                if matches!(self.workspace.panes.get(self.workspace.focused), Some(PaneKind::Editor(_))) {
                    return view::editor::focus(self.workspace.active);
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
                view::editor::focus(self.workspace.active)
            }
            Message::LinkClicked(uri) => {
                self.set_status(format!("link: {uri}"));
                Task::none()
            }

            Message::Workspace(message) => match message {
            WorkspaceMessage::PaneDragged(pane_grid::DragEvent::Dropped { pane, target }) => {
                self.workspace.panes.drop(pane, target);
                // The dragged pane keeps focus; re-derive `active` from
                // whatever now sits there.
                self.set_focus(self.workspace.focused);
                self.validate_panes();
                Task::none()
            }
            WorkspaceMessage::PaneDragged(_) => Task::none(),
            WorkspaceMessage::PaneResized(pane_grid::ResizeEvent { split, ratio }) => {
                self.workspace.panes.resize(split, ratio);
                Task::none()
            }
            WorkspaceMessage::PaneClicked(pane) => {
                self.ui.selection_menu = None;
                self.set_focus(pane);
                Task::none()
            }
            WorkspaceMessage::ToggleMaximize(pane) => {
                if self.workspace.panes.maximized() == Some(pane) {
                    self.workspace.panes.restore();
                } else {
                    self.workspace.panes.maximize(pane);
                }
                Task::none()
            }
            WorkspaceMessage::ToggleFullscreen(pane) => {
                if self.workspace.panes.get(pane).is_none() {
                    return Task::none();
                }
                let fullscreen = toggled_fullscreen(self.workspace.fullscreen, pane);
                self.set_focus(pane);
                self.workspace.fullscreen = fullscreen;
                if self.workspace.fullscreen.is_some() {
                    self.sidebar.close_context();
                    self.ui.close_editor_overlays();
                    self.ui.menu = None;
                }
                if matches!(self.workspace.panes.get(pane), Some(PaneKind::Editor(_))) {
                    view::editor::focus(self.workspace.active)
                } else {
                    Task::none()
                }
            }
            WorkspaceMessage::ClosePane(pane) => {
                match self.workspace.panes.get(pane) {
                    Some(PaneKind::Editor(id)) => {
                        let id = *id;
                        if self.workspace.documents.get(&id).is_some_and(|d| d.modified) {
                            // Confirm before discarding unsaved changes; the
                            // dialog takes over, so don't refocus the editor.
                            self.ui.confirm = Some(PendingAction::ClosePane(pane));
                            return Task::none();
                        }
                        self.close_pane(pane);
                    }
                    Some(PaneKind::Preview) => self.close_pane(pane),
                    None => return Task::none(),
                }
                // Closing moved focus to a sibling pane — give the editor the
                // keyboard back so the cursor is live without a click.
                view::editor::focus(self.workspace.active)
            }
            },

            Message::EscPressed => {
                // ESC peels UI layers: dialog, error, find bar, sub-bar, bar,
                // then opens the command bar.
                if self.ui.confirm.is_some() {
                    self.ui.confirm = None;
                } else if self.ui.selection_menu.is_some() {
                    self.ui.selection_menu = None;
                    return view::editor::focus(self.workspace.active);
                } else if self.ui.spell_correction.is_some() {
                    self.ui.spell_correction = None;
                    return view::editor::focus(self.workspace.active);
                } else if self.ui.open_picker {
                    self.ui.open_picker = false;
                    return view::editor::focus(self.workspace.active);
                } else if self.sidebar.context.is_some() {
                    self.sidebar.close_context();
                    return view::editor::focus(self.workspace.active);
                } else if self.active_doc().compile_error.is_some() {
                    self.active_doc_mut().compile_error = None;
                } else if self.ui.search.is_some() {
                    self.ui.search = None;
                    return view::editor::focus(self.workspace.active);
                } else if self.workspace.fullscreen.take().is_some() {
                    if matches!(self.workspace.panes.get(self.workspace.focused), Some(PaneKind::Editor(_))) {
                        return view::editor::focus(self.workspace.active);
                    }
                } else {
                    match self.ui.menu.take() {
                        // Root bar: close. Sub-views: back to the root bar.
                        Some(Menu::Commands(_)) => return view::editor::focus(self.workspace.active),
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
                match &mut self.ui.menu {
                    Some(Menu::Commands(picker)) => {
                        // halloy-style shortcut: "?" jumps to the keybinds.
                        if input.trim() == "?" {
                            self.ui.menu = Some(Menu::Help);
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
                match &mut self.ui.menu {
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
                let chosen = match &self.ui.menu {
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
                self.ui.menu = None;
                view::editor::focus(self.workspace.active)
            }
            Message::CompilerSelected(compiler) => {
                self.config.latex_compiler = compiler;
                self.save_config();
                self.ui.menu = None;
                view::editor::focus(self.workspace.active)
            }
            Message::FontSelected(name) => {
                self.config.editor_font = if name == core::config::BUILTIN_THEME {
                    String::new()
                } else {
                    name
                };
                self.editor_font = resolve_font(&self.config.editor_font);
                self.save_config();
                self.ui.menu = None;
                view::editor::focus(self.workspace.active)
            }
            Message::SplitRatioChanged(ratio) => {
                self.config.preview_split_ratio = ratio;
                if let Some(split) = self.preview_split {
                    self.workspace.panes.resize(split, ratio);
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
                view::editor::focus(self.workspace.active)
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
                view::editor::focus(self.workspace.active)
            }
            Message::SidebarContextChangeDirectory => {
                if let Some(path) = self.sidebar.context_directory() {
                    self.change_directory(path);
                }
                Task::none()
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
                    let id = self.workspace.insert_document(doc);
                    self.spawn_editor(id);
                    self.set_status(format!("created {name} — CTRL+S to save"));
                    view::editor::focus(self.workspace.active)
                }
            }

            Message::NewFile => {
                // A fresh scratch buffer in its own pane (CTRL+N).
                let id = self.workspace.insert_document(Document::untitled());
                self.spawn_editor(id);
                self.set_status("new file");
                view::editor::focus(self.workspace.active)
            }
            Message::OpenFilePicker => {
                self.sidebar.close_context();
                self.ui.open_picker = true;
                Task::none()
            }
            Message::ChooseFileToOpen => {
                self.ui.open_picker = false;
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
                self.ui.open_picker = false;
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
                self.ui.open_picker = false;
                view::editor::focus(self.workspace.active)
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
            Message::CloseActivePane => self.update(Message::Workspace(
                WorkspaceMessage::ClosePane(self.workspace.focused),
            )),
            Message::NextPane => self.cycle_pane(1),
            Message::PrevPane => self.cycle_pane(-1),
            Message::ToggleSearch => {
                if self.ui.search.take().is_some() {
                    // Second CTRL+F closes the bar and returns to the editor.
                    view::editor::focus(self.workspace.active)
                } else if self.ui.menu.is_some()
                    || self.ui.spell_correction.is_some()
                    || self.ui.selection_menu.is_some()
                    || self.ui.confirm.is_some()
                    || self.ui.open_picker
                {
                    Task::none() // don't pop find over a modal
                } else {
                    let origin = self.active_doc().cursor_pos();
                    self.ui.search = Some(Search {
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
                self.ui.search = None;
                view::editor::focus(self.workspace.active)
            }

            Message::CloseRequested => {
                if self.workspace.documents.values().any(|d| d.modified) {
                    self.ui.confirm = Some(PendingAction::CloseWindow);
                    Task::none()
                } else {
                    iced::exit()
                }
            }
            Message::ConfirmSave => {
                let Some(action) = self.ui.confirm.take() else {
                    return Task::none();
                };
                match action {
                    PendingAction::CloseWindow => {
                        // Save every modified document, then exit.
                        let mut failed = None;
                        for doc in self.workspace.documents.values_mut().filter(|d| d.modified) {
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
                        let id = match self.workspace.panes.get(pane) {
                            Some(PaneKind::Editor(id)) => *id,
                            _ => return Task::none(),
                        };
                        match self.workspace.documents.get_mut(&id).map(|d| d.save()) {
                            Some(Ok(_)) => {
                                self.close_pane(pane);
                                view::editor::focus(self.workspace.active)
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
                let Some(action) = self.ui.confirm.take() else {
                    return Task::none();
                };
                match action {
                    PendingAction::CloseWindow => iced::exit(),
                    PendingAction::ClosePane(pane) => {
                        self.close_pane(pane);
                        view::editor::focus(self.workspace.active)
                    }
                }
            }
            Message::ConfirmCancel => {
                self.ui.confirm = None;
                view::editor::focus(self.workspace.active)
            }
            Message::Tick => {
                if let Some((_, since)) = &self.ui.status
                    && since.elapsed() > STATUS_TTL
                {
                    self.ui.status = None;
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
    use iced::widget::pane_grid;

    use super::{PaneKind, toggled_fullscreen};

    #[test]
    fn fullscreen_toggles_and_follows_pane_focus() {
        let (mut panes, first) = pane_grid::State::new(PaneKind::Editor(0));
        let (_second, _) = panes
            .split(pane_grid::Axis::Vertical, first, PaneKind::Editor(1))
            .unwrap();

        assert_eq!(toggled_fullscreen(None, first), Some(first));
        assert_eq!(toggled_fullscreen(Some(first), first), None);
    }

}
