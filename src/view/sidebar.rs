//! The left sidebar: a collapsible directory tree plus a new-file input.
//!
//! The tree is a cached flat list ([`Sidebar::rebuild`]) — only expanded
//! directories are descended into, hidden entries are skipped, and the cache
//! refreshes on expand/collapse and after saves (new files appear once they
//! exist on disk).

use std::collections::BTreeSet;
use std::path::PathBuf;

use iced::widget::{button, column, container, row, scrollable, text, text_input};
use iced::{Background, Border, Element, Fill, Font, Padding};

use crate::app::{App, Message};

pub struct Sidebar {
    pub root: PathBuf,
    expanded: BTreeSet<PathBuf>,
    entries: Vec<Entry>,
    pub new_file: String,
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
        let mut sidebar = Self {
            root,
            expanded: BTreeSet::new(),
            entries: Vec::new(),
            new_file: String::new(),
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

pub fn view(app: &App) -> Element<'_, Message> {
    let theme = &app.theme;

    // Compare by canonical path: the tree builds paths like `./a.md` from
    // `read_dir(".")`, while a document opened from the CLI arg is just
    // `a.md` — raw `PathBuf` equality would miss those.
    let canon = |p: &std::path::Path| std::fs::canonicalize(p).ok();
    let open_paths: std::collections::BTreeSet<PathBuf> =
        app.open_paths().iter().filter_map(|p| canon(p)).collect();
    let active_path = app.active_doc().path.as_deref().and_then(canon);

    let mut tree = column![].spacing(1);
    for entry in &app.sidebar.entries {
        let entry_canon = (!entry.is_dir).then(|| canon(&entry.path)).flatten();
        let is_open = entry_canon
            .as_ref()
            .is_some_and(|c| open_paths.contains(c));
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
        tree = tree.push(
            button(text(label).size(13).color(color))
                .on_press(message)
                .padding(Padding {
                    left: 8.0 + f32::from(entry.depth) * 14.0,
                    right: 8.0,
                    top: 3.0,
                    bottom: 3.0,
                })
                .width(Fill)
                .style(move |_theme, status| {
                    entry_button(color, accent, is_open, is_active, status)
                }),
        );
    }

    // Resolve "." to an absolute path so the header shows the real folder.
    let dir_name = std::fs::canonicalize(&app.sidebar.root)
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| app.sidebar.root.display().to_string());
    let header = column![
        text("📁 Current directory:")
            .size(11)
            .color(theme.text_inactive),
        text(dir_name).size(14).color(theme.text),
    ]
    .spacing(2)
    .padding(Padding::from([0.0, 8.0]));

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
            vec![
                ("⇥", "accept"),
                ("⌃⌫", "drop last word"),
                ("⇧⌫", "discard"),
            ],
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
    let hovered = matches!(
        status,
        button::Status::Hovered | button::Status::Pressed
    );
    let white = |a| iced::Color { a, ..iced::Color::WHITE };
    let background = if is_active {
        Some(iced::Color { a: if hovered { 0.30 } else { 0.22 }, ..accent })
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
