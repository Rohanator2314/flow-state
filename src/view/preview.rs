//! The preview pane: rendered markdown, or a continuously scrollable column
//! of PDF pages.
//!
//! The PDF pages are stacked top-to-bottom inside a `scrollable`, so a plain
//! wheel scrolls smoothly between them. Holding CTRL switches the wheel to
//! zoom: in that state the view wraps the pages in a `mouse_area` whose
//! `on_scroll` captures the wheel (stopping the scrollable from also moving)
//! and feeds it to `Message::PdfScroll`. Each page is scaled to the pane
//! width times the zoom factor, keeping its aspect ratio.

use iced::widget::{center, column, container, image, markdown, mouse_area, responsive, scrollable, text};
use iced::{Element, Fill};

use crate::app::{App, Message, Preview};
use crate::core::FileKind;

/// Vertical gap between stacked PDF pages.
const PAGE_GAP: f32 = 8.0;

pub fn view(app: &App) -> Element<'_, Message> {
    match &app.active_doc().preview {
        Preview::None => {
            let hint = match app.active_doc().kind() {
                FileKind::Latex => "CTRL+S to compile & preview",
                FileKind::Markdown => "CTRL+S to preview",
                FileKind::Plain => "",
            };
            center(text(hint).color(app.theme.text_inactive)).into()
        }
        Preview::Markdown(content) => scrollable(
            container(
                markdown::view(
                    content.items(),
                    markdown::Settings::from(app.theme.iced_theme()),
                )
                .map(Message::LinkClicked),
            )
            .padding(20),
        )
        .height(Fill)
        .into(),
        Preview::Pdf(pages) => pdf(app, pages),
    }
}

fn pdf<'a>(app: &'a App, pages: &'a [crate::app::PdfPage]) -> Element<'a, Message> {
    let zoom = app.pdf_zoom;
    let ctrl = app.ctrl_held;

    // `responsive` hands us the pane width so a zoom of 1.0 fits the page to
    // the pane; higher zoom overflows and the scrollable pans horizontally.
    let body = responsive(move |size| {
        let page_w = (size.width - 2.0 * PAGE_GAP).max(1.0) * zoom;
        let stacked = pages.iter().fold(
            column![].spacing(PAGE_GAP).align_x(iced::Center),
            |col, page| {
                col.push(
                    image(page.handle.clone())
                        .width(page_w)
                        .height(page_w * page.aspect),
                )
            },
        );

        // Only intercept the wheel for zoom while CTRL is held; otherwise let
        // the scrollable handle it natively for smooth page scrolling.
        let content: Element<'_, Message> = if ctrl {
            mouse_area(stacked).on_scroll(Message::PdfScroll).into()
        } else {
            stacked.into()
        };

        scrollable(container(content).padding(PAGE_GAP).center_x(Fill))
            .height(Fill)
            .into()
    });

    container(body).height(Fill).into()
}
