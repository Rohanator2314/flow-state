//! Transient interface state, kept separate from documents and workspace.

use std::time::Instant;

use crate::app::{Menu, PendingAction, Search, SpellCorrection};
use crate::selection::SelectionMenu;

#[derive(Default)]
pub struct UiState {
    pub(crate) confirm: Option<PendingAction>,
    pub(crate) open_picker: bool,
    pub(crate) search: Option<Search>,
    pub(crate) spell_correction: Option<SpellCorrection>,
    pub(crate) selection_menu: Option<SelectionMenu>,
    pub(crate) menu: Option<Menu>,
    pub(crate) status: Option<(String, Instant)>,
}

impl UiState {
    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status = Some((message.into(), Instant::now()));
    }

    pub fn close_editor_overlays(&mut self) {
        self.search = None;
        self.spell_correction = None;
        self.selection_menu = None;
    }

    pub fn open_selection_menu(&mut self, menu: SelectionMenu) {
        self.search = None;
        self.spell_correction = None;
        self.menu = None;
        self.selection_menu = Some(menu);
    }
}
