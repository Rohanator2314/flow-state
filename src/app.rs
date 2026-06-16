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
use std::time::{Duration, Instant, SystemTime};

use iced::widget::{image, markdown, pane_grid, text_editor};
use iced::{window, Element, Subscription, Task, Theme};

use crate::core::config::Config;
use crate::core::theme::Theme as FlowTheme;
use crate::core::undo::{History, Snapshot};
use crate::core::{self, file_kind, text, FileKind};
use crate::view::{self, sidebar::Sidebar};

/// How long transient status-bar messages stay visible.
const STATUS_TTL: Duration = Duration::from_secs(4);

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
    NextParagraph,
    PrevParagraph,
    // preview
    Compiled(DocId, Result<Vec<PdfPage>, String>),
    PdfScroll(iced::mouse::ScrollDelta),
    CtrlChanged(bool),
    DismissError,
    LinkClicked(markdown::Uri),
    // panes
    PaneDragged(pane_grid::DragEvent),
    PaneResized(pane_grid::ResizeEvent),
    PaneClicked(pane_grid::Pane),
    ToggleMaximize(pane_grid::Pane),
    ClosePane(pane_grid::Pane),
    // sidebar
    ToggleDir(PathBuf),
    OpenFile(PathBuf),
    NewFileInput(String),
    CreateFile,
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

/// Root command-bar entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Theme,
    Font,
    Compiler,
    Split,
    Dimming,
    Help,
}

impl Command {
    const ALL: [Command; 6] = [
        Command::Theme,
        Command::Font,
        Command::Compiler,
        Command::Split,
        Command::Dimming,
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
    use iced::keyboard::{key::Named, Event, Key};
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
    use iced::keyboard::{key::Named, Event, Key};
    let iced::Event::Keyboard(Event::KeyPressed { key, .. }) = event else {
        return None;
    };
    match key {
        Key::Named(Named::ArrowUp) => Some(Message::MenuPrev),
        Key::Named(Named::ArrowDown) => Some(Message::MenuNext),
        _ => None,
    }
}

/// Subscription filter: track the CTRL modifier (for PDF wheel zoom).
fn on_ctrl(
    event: iced::Event,
    _status: iced::event::Status,
    _window: window::Id,
) -> Option<Message> {
    use iced::keyboard::Event;
    match event {
        iced::Event::Keyboard(Event::ModifiersChanged(m)) => {
            Some(Message::CtrlChanged(m.control()))
        }
        _ => None,
    }
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
        }
    }

    fn open(path: PathBuf) -> Self {
        let content = match std::fs::read_to_string(&path) {
            Ok(text) => text_editor::Content::with_text(&text),
            Err(_) => text_editor::Content::new(),
        };
        let mut doc = Self {
            path: Some(path),
            content,
            history: History::default(),
            modified: false,
            generation: 0,
            preview: Preview::None,
            compiling: false,
            compile_error: None,
        };
        // `with_text` leaves the cursor at the end; start at the top.
        doc.move_to((0, 0));
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
        let path = self
            .path
            .clone()
            .ok_or("no filename — create one in the sidebar")?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut text = self.content.text();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        self.modified = false;
        Ok(path)
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
    /// Whether CTRL is currently held — switches PDF wheel from scroll to
    /// zoom (tracked from keyboard modifier events).
    pub ctrl_held: bool,
    pub panes: pane_grid::State<PaneKind>,
    /// The pane that last received a click; gets the highlighted border.
    pub focused: pane_grid::Pane,
    pub sidebar: Sidebar,
    pub confirm: Option<PendingAction>,
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
            ctrl_held: false,
            panes,
            focused: first,
            sidebar: Sidebar::new(PathBuf::from(".")),
            confirm: None,
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
        (app, focus)
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
        ];
        if self.menu.is_some() {
            // The command bar's filter input ignores arrow keys, so they
            // arrive here and drive the list selection.
            subs.push(iced::event::listen_with(on_menu_arrows));
        }
        if matches!(self.active_doc().preview, Preview::Pdf(_)) {
            // Track CTRL so the PDF wheel can switch between scroll and zoom.
            subs.push(iced::event::listen_with(on_ctrl));
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
            files.push(dir.join("themes").join(format!("{}.toml", self.config.theme)));
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
    fn poll_config(&mut self) {
        if self.menu.is_some() {
            return;
        }
        let sig = self.config_signature();
        if sig == self.config_sig {
            return;
        }
        self.config_sig = sig;

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
        if matches!(self.panes.get(pane), Some(PaneKind::Editor(_)))
            && self.editor_count() <= 1
        {
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

        let doc = Document::open(path);
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
                self.active_doc_mut().preview =
                    Preview::Markdown(markdown::Content::parse(&text));
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
                                core::latex::compile(&compiler, &path).map(|pages| {
                                    pages.into_iter().map(to_page).collect::<Vec<_>>()
                                })
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
        match message {
            Message::Edit(id, action) => {
                // An edit/click in a pane makes its document the focused one.
                if let Some(pane) = self.pane_of_doc(id) {
                    self.set_focus(pane);
                }
                let Some(doc) = self.docs.get_mut(&id) else {
                    return Task::none();
                };
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
                Task::none()
            }
            Message::DeleteSentence => {
                let doc = self.active_doc_mut();
                let lines = doc.lines();
                let pos = doc.content.cursor().position;
                let cursor = (pos.line, pos.column);
                if let Some(start) = text::sentence_start_before(&lines, cursor)
                    && start != cursor
                {
                    doc.history.record(doc.snapshot(), false);
                    doc.modified = true;
                    doc.content.move_to(text_editor::Cursor {
                        position: pos,
                        selection: Some(text_editor::Position {
                            line: start.0,
                            column: start.1,
                        }),
                    });
                    doc.content
                        .perform(text_editor::Action::Edit(text_editor::Edit::Backspace));
                }
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
                Task::none()
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
            Message::CtrlChanged(held) => {
                self.ctrl_held = held;
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
                Task::none()
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
                            // Confirm before discarding unsaved changes.
                            self.confirm = Some(PendingAction::ClosePane(pane));
                        } else {
                            self.close_pane(pane);
                        }
                    }
                    Some(PaneKind::Preview) => self.close_pane(pane),
                    None => {}
                }
                Task::none()
            }

            Message::EscPressed => {
                // ESC peels UI layers: dialog, error, sub-bar, bar, then opens.
                if self.confirm.is_some() {
                    self.confirm = None;
                } else if self.active_doc().compile_error.is_some() {
                    self.active_doc_mut().compile_error = None;
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

            Message::ToggleDir(path) => {
                self.sidebar.toggle(path);
                Task::none()
            }
            Message::OpenFile(path) => self.open_file(path),
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
                let path = self.sidebar.root.join(&name);
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
                                Task::none()
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
                        Task::none()
                    }
                }
            }
            Message::ConfirmCancel => {
                self.confirm = None;
                Task::none()
            }
            Message::Tick => {
                if let Some((_, since)) = &self.status
                    && since.elapsed() > STATUS_TTL
                {
                    self.status = None;
                }
                self.poll_config();
                Task::none()
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
