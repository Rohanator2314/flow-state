//! Rendering: the top-level layout and each visual region.
//!
//! `view(app)` lays out sidebar | pane grid, with a one-row status bar at the
//! bottom and any open dialog stacked on top.
//!
//! The pane chrome (gaps, rounded 1px-bordered cards, focus-highlighted
//! border, title bar with 22×22 control buttons) follows halloy's dashboard
//! panes (GPL-3.0, <https://github.com/squidowl/halloy>,
//! `src/screen/dashboard/pane.rs` + `src/appearance/theme/container.rs>`),
//! adapted to our theme model: the area behind the panes is the chrome
//! `surface` color, pane bodies are the editor `background`, and the focused
//! pane's border uses the theme accent.

pub mod dialogs;
pub mod editor;
pub mod menu;
pub mod preview;
pub mod sidebar;
pub mod style;

use iced::border::Radius;
use iced::widget::{button, center, column, container, pane_grid, row, text};
use iced::{Background, Border, Color, Element, Fill, Padding};

use crate::app::{App, Message, PaneKind};

/// Gap between panes and around the grid (halloy's inner/outer pane gap).
const GAP: f32 = 6.0;

pub fn view(app: &App) -> Element<'_, Message> {
    let theme = &app.theme;

    let grid = pane_grid(&app.panes, |pane, kind, maximized| {
        let is_focused = pane == app.focused;
        let body: Element<'_, Message> = match kind {
            PaneKind::Editor(id) => editor::view(app, *id),
            PaneKind::Preview => preview::view(app),
        };
        pane_grid::Content::new(body)
            .style(move |_| pane_body(theme, is_focused))
            .title_bar(title_bar(app, *kind, pane, maximized, is_focused))
    })
    .spacing(GAP)
    .on_click(Message::PaneClicked)
    .on_drag(Message::PaneDragged)
    .on_resize(8, Message::PaneResized);

    let shell = container(grid)
        .padding(Padding::new(GAP))
        .style(move |_| container::Style {
            background: Some(Background::Color(theme.surface)),
            ..container::Style::default()
        });

    let base: Element<'_, Message> = column![
        row![sidebar::view(app), shell].height(Fill),
        status_bar(app)
    ]
    .into();

    if let Some(pending) = &app.confirm {
        return dialogs::modal(base, dialogs::confirm(app, pending));
    }
    if let Some(error) = &app.active_doc().compile_error {
        return dialogs::modal(base, dialogs::compile_error(app, error));
    }
    if let Some(menu) = &app.menu {
        return dialogs::modal_top(base, menu::view(app, menu));
    }
    base
}

/// Pane body card: rounded, 1px border, highlighted when focused.
/// (halloy: `theme::container::buffer`)
fn pane_body(theme: &crate::core::theme::Theme, focused: bool) -> container::Style {
    container::Style {
        background: Some(Background::Color(theme.background)),
        border: Border {
            color: if focused { theme.accent } else { theme.border },
            width: 1.0,
            radius: Radius {
                top_left: 4.0,
                top_right: 4.0,
                bottom_right: 4.0,
                bottom_left: 4.0,
            },
        },
        ..container::Style::default()
    }
}

/// Title bar: top-rounded strip in its own shade, with the pane title and
/// the control buttons. (halloy: `theme::container::buffer_title_bar`)
fn title_bar<'a>(
    app: &App,
    kind: PaneKind,
    pane: pane_grid::Pane,
    maximized: bool,
    is_focused: bool,
) -> pane_grid::TitleBar<'a, Message> {
    let theme = &app.theme;
    let title_text = match kind {
        PaneKind::Editor(id) => {
            let doc = &app.docs[&id];
            let dot = if doc.modified { " ●" } else { "" };
            format!("{}{dot}", doc.display_name())
        }
        PaneKind::Preview => "preview".to_string(),
    };
    let title_color = if is_focused { theme.text } else { theme.text_inactive };
    let dim = theme.text_inactive;

    let mut controls = row![control_button(
        if maximized { "🗗" } else { "🗖" },
        Message::ToggleMaximize(pane),
        dim,
        theme,
    )]
    .spacing(2);
    // The preview pane closes (and reopens on save); editor panes close once
    // there's more than one (there must always be a document to edit).
    let closable = match kind {
        PaneKind::Preview => true,
        PaneKind::Editor(_) => app.editor_count() > 1,
    };
    if closable {
        controls = controls.push(control_button("✕", Message::ClosePane(pane), dim, theme));
    }

    let bar_bg = mix(theme.surface, theme.background);
    let border_color = if is_focused { theme.accent } else { theme.border };
    pane_grid::TitleBar::new(
        container(text(title_text).size(14).color(title_color))
            .height(22)
            .padding([0, 4])
            .align_y(iced::Center),
    )
    .controls(pane_grid::Controls::new(controls))
    .padding(6)
    .style(move |_| container::Style {
        background: Some(Background::Color(bar_bg)),
        border: Border {
            color: border_color,
            width: 1.0,
            radius: Radius {
                top_left: 4.0,
                top_right: 4.0,
                bottom_right: 0.0,
                bottom_left: 0.0,
            },
        },
        ..container::Style::default()
    })
}

/// A 22×22 title-bar control button (halloy's pane control dimensions).
fn control_button<'a>(
    icon: &'a str,
    message: Message,
    color: Color,
    theme: &crate::core::theme::Theme,
) -> iced::widget::Button<'a, Message> {
    button(center(text(icon).size(11).color(color)))
        .padding(2)
        .width(22)
        .height(22)
        .on_press(message)
        .style(style::bare_button(theme))
}

/// Midpoint of two colors, for the title-bar shade.
fn mix(a: Color, b: Color) -> Color {
    Color {
        r: (a.r + b.r) / 2.0,
        g: (a.g + b.g) / 2.0,
        b: (a.b + b.b) / 2.0,
        a: 1.0,
    }
}

fn status_bar(app: &App) -> Element<'_, Message> {
    let theme = &app.theme;
    let doc = app.active_doc();
    let modified = if doc.modified { " ●" } else { "" };
    let mut left = format!(" {}{modified}", doc.display_name());
    if doc.compiling {
        left.push_str("   ⟳ compiling…");
    }
    if let Some((msg, _)) = &app.status {
        left.push_str("   ");
        left.push_str(msg);
    }

    let (current, total) = paragraph_info(app);
    let right = format!("¶ {current}/{total} ");

    container(
        row![
            text(left).size(12),
            iced::widget::space().width(Fill),
            text(right).size(12),
        ]
        .padding([3, 8]),
    )
    .width(Fill)
    .style(move |_| container::Style {
        background: Some(Background::Color(theme.surface)),
        text_color: Some(theme.surface_text),
        ..container::Style::default()
    })
    .into()
}

/// (1-based index of the cursor's paragraph, total paragraph count).
fn paragraph_info(app: &App) -> (usize, usize) {
    let content = &app.active_doc().content;
    let cursor_line = content.cursor().position.line;
    let mut total = 0;
    let mut current = 0;
    let mut in_paragraph = false;
    for i in 0..content.line_count() {
        let blank = content
            .line(i)
            .is_none_or(|l| l.text.chars().all(char::is_whitespace));
        if !blank && !in_paragraph {
            total += 1;
        }
        in_paragraph = !blank;
        if i == cursor_line && in_paragraph {
            current = total;
        }
    }
    (current, total)
}
