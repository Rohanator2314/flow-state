//! The left sidebar: a collapsible directory tree plus a new-file input.
//!
//! The tree is a cached flat list ([`Sidebar::rebuild`]) — only expanded
//! directories are descended into, hidden entries are skipped, and the cache
//! refreshes on expand/collapse and after saves (new files appear once they
//! exist on disk).

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use iced::widget::{button, column, container, mouse_area, row, scrollable, text, text_input};
use iced::{Background, Border, Element, Fill, Font, Padding};

use crate::app::{App, Message};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextMode {
    Menu,
    CreateFile,
    CreateFolder,
    Rename,
    ConfirmDelete,
}

pub struct ContextMenu {
    pub target: PathBuf,
    pub is_dir: bool,
    pub mode: ContextMode,
    pub input: String,
}

pub struct Sidebar {
    pub root: PathBuf,
    pub collapsed: bool,
    expanded: BTreeSet<PathBuf>,
    entries: Vec<Entry>,
    pub new_file: String,
    pub context: Option<ContextMenu>,
}

struct Entry {
    path: PathBuf,
    name: String,
    depth: u16,
    is_dir: bool,
    expanded: bool,
}

impl Sidebar {
    pub fn new(root: PathBuf) -> Self {
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        let mut sidebar = Self {
            root,
            collapsed: false,
            expanded: BTreeSet::new(),
            entries: Vec::new(),
            new_file: String::new(),
            context: None,
        };
        sidebar.rebuild();
        sidebar
    }

    pub fn toggle(&mut self, path: PathBuf) {
        if !self.expanded.remove(&path) {
            self.expanded.insert(path);
        }
        self.rebuild();
    }

    pub fn toggle_collapsed(&mut self) {
        self.collapsed = !self.collapsed;
    }

    pub fn set_root(&mut self, path: PathBuf) -> Result<(), String> {
        let root = std::fs::canonicalize(&path).map_err(|e| e.to_string())?;
        if !root.is_dir() {
            return Err(format!("{} is not a folder", root.display()));
        }
        self.root = root;
        self.expanded.clear();
        self.context = None;
        self.rebuild();
        Ok(())
    }

    pub fn open_context(&mut self, target: PathBuf, is_dir: bool) {
        self.context = Some(ContextMenu {
            target,
            is_dir,
            mode: ContextMode::Menu,
            input: String::new(),
        });
    }

    pub fn close_context(&mut self) {
        self.context = None;
    }

    pub fn context_directory(&self) -> Option<PathBuf> {
        self.context
            .as_ref()
            .filter(|context| context.is_dir)
            .map(|context| context.target.clone())
    }

    pub fn context_is_editing(&self) -> bool {
        self.context.as_ref().is_some_and(|context| {
            matches!(
                context.mode,
                ContextMode::CreateFile | ContextMode::CreateFolder | ContextMode::Rename
            )
        })
    }

    pub fn rebuild(&mut self) {
        self.entries.clear();
        let root = self.root.clone();
        self.walk(&root, 0);
    }

    fn walk(&mut self, dir: &std::path::Path, depth: u16) {
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        let mut items: Vec<(PathBuf, String, bool)> = read
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    return None;
                }
                let is_dir = e.file_type().ok()?.is_dir();
                Some((e.path(), name, is_dir))
            })
            .collect();
        // Directories first, then alphabetical — the usual file-tree order.
        items.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(&b.1)));

        for (path, name, is_dir) in items {
            let expanded = is_dir && self.expanded.contains(&path);
            self.entries.push(Entry {
                path: path.clone(),
                name,
                depth,
                is_dir,
                expanded,
            });
            if expanded {
                self.walk(&path, depth + 1);
            }
        }
    }
}

fn context_input_id() -> iced::widget::Id {
    iced::widget::Id::from("sidebar-context-input")
}

pub fn focus_context_input() -> iced::Task<Message> {
    iced::widget::operation::focus(context_input_id())
}

/// Join one plain filename to `parent`, rejecting empty names and traversal.
pub fn child_path(parent: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(parent.join(name)),
        _ => Err("enter a single file or folder name".to_string()),
    }
}

/// Move a document path along with a renamed file or directory.
pub fn remap_descendant(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    path.strip_prefix(from).ok().map(|tail| to.join(tail))
}

pub fn view(app: &App) -> Element<'_, Message> {
    let theme = &app.theme;

    if app.sidebar.collapsed {
        let expand = button(text("›").size(16).color(theme.text_inactive))
            .on_press(Message::ToggleSidebar)
            .padding([2, 8])
            .style(crate::view::style::bare_button(theme));
        return container(
            container(expand)
                .width(Fill)
                .align_x(iced::Right),
        )
        .width(34)
        .height(Fill)
        .padding(Padding::new(4.0).top(10.0))
        .style(move |_| container::Style {
            background: Some(Background::Color(theme.surface)),
            ..container::Style::default()
        })
        .into();
    }

    // Compare by canonical path: the tree builds paths like `./a.md` from
    // `read_dir(".")`, while a document opened from the CLI arg is just
    // `a.md` — raw `PathBuf` equality would miss those.
    let canon = |p: &std::path::Path| std::fs::canonicalize(p).ok();
    let open_paths: std::collections::BTreeSet<PathBuf> =
        app.open_paths().iter().filter_map(|p| canon(p)).collect();
    let active_path = app.active_doc().path.as_deref().and_then(canon);

    let mut tree = column![].spacing(1);
    if app
        .sidebar
        .context
        .as_ref()
        .is_some_and(|context| context.target == app.sidebar.root)
    {
        tree = tree.push(context_menu(app));
    }
    for entry in &app.sidebar.entries {
        let entry_canon = (!entry.is_dir).then(|| canon(&entry.path)).flatten();
        let is_open = entry_canon.as_ref().is_some_and(|c| open_paths.contains(c));
        let is_active = entry_canon.is_some() && entry_canon == active_path;

        let label = if entry.is_dir {
            format!("{} {}", if entry.expanded { "▾" } else { "▸" }, entry.name)
        } else {
            entry.name.clone()
        };
        // The focused file is brightest; other open files stay readable.
        let color = if is_active {
            theme.accent
        } else if is_open {
            theme.text
        } else {
            theme.text_inactive
        };
        let message = if entry.is_dir {
            Message::ToggleDir(entry.path.clone())
        } else {
            Message::OpenFile(entry.path.clone())
        };
        let accent = theme.accent;
        let entry_button = button(text(label).size(13).color(color))
            .on_press(message)
            .padding(Padding {
                left: 8.0 + f32::from(entry.depth) * 14.0,
                right: 8.0,
                top: 3.0,
                bottom: 3.0,
            })
            .width(Fill)
            .style(move |_theme, status| entry_button(color, accent, is_open, is_active, status));
        let entry_area = mouse_area(entry_button).on_right_press(Message::OpenSidebarContext(
            entry.path.clone(),
            entry.is_dir,
        ));
        tree = tree.push(entry_area);

        if app
            .sidebar
            .context
            .as_ref()
            .is_some_and(|context| context.target == entry.path)
        {
            tree = tree.push(context_menu(app));
        }
    }

    // Resolve "." to an absolute path so the header shows the real folder.
    let dir_name = std::fs::canonicalize(&app.sidebar.root)
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| app.sidebar.root.display().to_string());
    let mut header_row = row![text(dir_name).size(14).color(theme.text)].spacing(6);
    if let Some(parent) = app.sidebar.root.parent() {
        header_row = header_row.push(
            button(text("↑").size(13))
                .on_press(Message::ChangeDirectory(parent.to_path_buf()))
                .padding([1, 6])
                .style(crate::view::style::bare_button(theme)),
        );
    }
    let directory = mouse_area(
        column![
            text("Current directory:")
                .size(11)
                .color(theme.text_inactive),
            header_row,
        ]
        .spacing(2)
        .padding(Padding::from([0.0, 8.0])),
    )
    .on_right_press(Message::OpenSidebarContext(app.sidebar.root.clone(), true));
    let collapse = button(text("‹").size(16).color(theme.text_inactive))
        .on_press(Message::ToggleSidebar)
        .padding([2, 8])
        .style(crate::view::style::bare_button(theme));
    let header =
        row![container(directory).width(Fill), collapse].align_y(iced::Alignment::Start);

    let new_file = text_input("new file…", &app.sidebar.new_file)
        .on_input(Message::NewFileInput)
        .on_submit(Message::CreateFile)
        .size(13)
        .padding(8);

    container(
        column![
            header,
            scrollable(tree).height(Fill),
            keybind_hints(app),
            new_file
        ]
        .spacing(8),
    )
    .width(230)
    .height(Fill)
    .padding(Padding::new(6.0).top(10.0))
    .style(move |_| container::Style {
        background: Some(Background::Color(theme.surface)),
        ..container::Style::default()
    })
    .into()
}

fn context_menu(app: &App) -> Element<'_, Message> {
    let theme = &app.theme;
    let context = app.sidebar.context.as_ref().expect("context menu exists");
    let target_name = context
        .target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| context.target.display().to_string());
    let target_kind = if context.is_dir { "FOLDER" } else { "FILE" };
    let body: Element<'_, Message> = match context.mode {
        ContextMode::Menu => {
            let mut actions = column![].spacing(1);
            if context.is_dir && context.target != app.sidebar.root {
                actions = actions.push(
                    button(text("Change current directory").size(12))
                        .on_press(Message::SidebarContextChangeDirectory)
                        .padding([5, 8])
                        .width(Fill)
                        .style(crate::view::style::action_button(theme, false)),
                );
            }
            actions = actions
                .push(
                    button(text("New file").size(12))
                        .on_press(Message::SidebarContextCreateFile)
                        .padding([5, 8])
                        .width(Fill)
                        .style(crate::view::style::action_button(theme, false)),
                )
                .push(
                    button(text("New folder").size(12))
                        .on_press(Message::SidebarContextCreateFolder)
                        .padding([5, 8])
                        .width(Fill)
                        .style(crate::view::style::action_button(theme, false)),
                );
            if context.target != app.sidebar.root {
                actions = actions
                    .push(
                        button(text("Rename").size(12))
                            .on_press(Message::SidebarContextRename)
                            .padding([5, 8])
                            .width(Fill)
                            .style(crate::view::style::action_button(theme, false)),
                    )
                    .push(
                        button(text("Delete").size(12))
                            .on_press(Message::SidebarContextDelete)
                            .padding([5, 8])
                            .width(Fill)
                            .style(crate::view::style::action_button(theme, true)),
                    );
            }
            actions.into()
        }
        ContextMode::CreateFile | ContextMode::CreateFolder | ContextMode::Rename => {
            let placeholder = match context.mode {
                ContextMode::CreateFile => "new file name",
                ContextMode::CreateFolder => "new folder name",
                ContextMode::Rename => "new name",
                _ => unreachable!(),
            };
            column![
                text_input(placeholder, &context.input)
                    .id(context_input_id())
                    .on_input(Message::SidebarContextInput)
                    .on_submit(Message::SidebarContextSubmit)
                    .size(12)
                    .padding(6),
                row![
                    button(text("Confirm").size(12))
                        .on_press(Message::SidebarContextSubmit)
                        .style(button::primary),
                    button(text("Cancel").size(12))
                        .on_press(Message::CloseSidebarContext)
                        .style(button::secondary),
                ]
                .spacing(5),
            ]
            .spacing(5)
            .into()
        }
        ContextMode::ConfirmDelete => column![
            text(if context.is_dir {
                format!("Delete {target_name} and all its contents?")
            } else {
                format!("Delete {target_name}?")
            })
            .size(12)
            .color(theme.danger),
            row![
                button(text("Delete").size(12))
                    .on_press(Message::SidebarContextConfirmDelete)
                    .style(button::danger),
                button(text("Cancel").size(12))
                    .on_press(Message::CloseSidebarContext)
                    .style(button::secondary),
            ]
            .spacing(5),
        ]
        .spacing(5)
        .into(),
    };

    container(
        column![
            column![
                text(target_kind).size(10).color(theme.text_inactive),
                text(target_name).size(12).color(theme.text),
            ]
            .spacing(1),
            body,
        ]
        .spacing(7),
    )
    .padding(8)
    .width(Fill)
    .style(crate::view::style::inline_panel(theme))
    .into()
}

/// A live keybind cheat-sheet shown just above the new-file input. It reacts
/// to the held-modifier set — holding CTRL/SHIFT/ALT reveals exactly the
/// bindings that need that key — and, while a phantom is active, lists the
/// phantom controls instead. Pairs with the in-editor accent emphasis: the
/// word/sentence a BACKSPACE would hit is highlighted as the keys appear here.
fn keybind_hints(app: &App) -> Element<'_, Message> {
    let theme = &app.theme;
    let m = app.modifiers;
    let phantom = app.active_doc().phantom.is_some();

    let (heading, rows): (&str, Vec<(&str, &str)>) = if phantom {
        (
            "phantom",
            vec![("⇥", "accept"), ("⌃⌫", "drop last word"), ("⇧⌫", "discard")],
        )
    } else if m.control() && m.shift() {
        ("⌃⇧", vec![("⌃⇧Z", "redo")])
    } else if m.control() {
        (
            "⌃ held",
            vec![
                ("⌃S", "save & preview"),
                ("⌃Z", "undo"),
                ("⌃Y", "redo"),
                ("⌃H/J/K/L", "move ←/↓/↑/→"),
                ("⌃.", "next / correct spelling"),
                ("⌃⌫", "delete word"),
            ],
        )
    } else if m.shift() {
        ("⇧ held", vec![("⇧⌫", "delete sentence")])
    } else if m.alt() {
        (
            "⌥ held",
            vec![
                ("⌥W / ⌥B", "next / prev word"),
                ("⌥N / ⌥⇧N", "next / prev paragraph"),
            ],
        )
    } else {
        (
            "hold a key",
            vec![
                ("⌃", "save · undo · word"),
                ("⇧", "delete sentence"),
                ("⌥", "word · paragraph nav"),
            ],
        )
    };

    let mut list = column![text(heading).size(10).color(theme.text_inactive)].spacing(3);
    for (key, action) in rows {
        list = list.push(
            row![
                text(key)
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(theme.accent)
                    .width(70),
                text(action).size(11).color(theme.text_inactive),
            ]
            .spacing(6),
        );
    }

    container(list)
        .width(Fill)
        .padding(Padding::from([8.0, 10.0]))
        .style(move |_| container::Style {
            background: Some(Background::Color(theme.background)),
            border: Border::default().rounded(6),
            ..container::Style::default()
        })
        .into()
}

/// A sidebar entry button. Open files get a background tint so they stand out
/// at a glance — the focused file an accent fill, other open files a faint
/// neutral fill — while hover always lifts the row a little more.
fn entry_button(
    color: iced::Color,
    accent: iced::Color,
    is_open: bool,
    is_active: bool,
    status: button::Status,
) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let white = |a| iced::Color {
        a,
        ..iced::Color::WHITE
    };
    let background = if is_active {
        Some(iced::Color {
            a: if hovered { 0.30 } else { 0.22 },
            ..accent
        })
    } else if is_open {
        Some(white(if hovered { 0.10 } else { 0.05 }))
    } else if hovered {
        Some(white(0.06))
    } else {
        None
    };
    button::Style {
        background: background.map(Background::Color),
        text_color: color,
        border: Border::default().rounded(4),
        ..button::Style::default()
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{Sidebar, child_path, remap_descendant};

    #[test]
    fn collapse_state_toggles_and_is_reversible() {
        let mut sidebar = Sidebar::new(PathBuf::new());
        assert!(!sidebar.collapsed);

        sidebar.toggle_collapsed();
        assert!(sidebar.collapsed);

        sidebar.toggle_collapsed();
        assert!(!sidebar.collapsed);
    }

    #[test]
    fn only_folder_contexts_offer_a_directory_target() {
        let mut sidebar = Sidebar::new(PathBuf::new());
        let folder = PathBuf::from("notes");
        sidebar.open_context(folder.clone(), true);
        assert_eq!(sidebar.context_directory(), Some(folder));

        sidebar.open_context(PathBuf::from("notes.md"), false);
        assert_eq!(sidebar.context_directory(), None);
    }

    #[test]
    fn child_path_accepts_only_one_plain_name() {
        assert_eq!(
            child_path(Path::new("/work"), " notes.md ").unwrap(),
            Path::new("/work/notes.md")
        );
        assert!(child_path(Path::new("/work"), "../notes.md").is_err());
        assert!(child_path(Path::new("/work"), "a/b").is_err());
        assert!(child_path(Path::new("/work"), "").is_err());
    }

    #[test]
    fn remap_descendant_preserves_relative_tail() {
        assert_eq!(
            remap_descendant(
                Path::new("/work/old/chapter/one.md"),
                Path::new("/work/old"),
                Path::new("/work/new"),
            ),
            Some(Path::new("/work/new/chapter/one.md").to_path_buf())
        );
        assert_eq!(
            remap_descendant(
                Path::new("/work/other.md"),
                Path::new("/work/old"),
                Path::new("/work/new"),
            ),
            None
        );
    }

    #[test]
    fn changing_root_rebuilds_from_the_selected_folder() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("flow-state-sidebar-{unique}"));
        let nested = base.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("visible.txt"), "test").unwrap();

        let mut sidebar = Sidebar::new(base.clone());
        sidebar.set_root(nested.clone()).unwrap();

        assert_eq!(sidebar.root, std::fs::canonicalize(&nested).unwrap());
        assert_eq!(sidebar.entries.len(), 1);
        assert_eq!(sidebar.entries[0].name, "visible.txt");

        std::fs::remove_dir_all(base).unwrap();
    }
}
