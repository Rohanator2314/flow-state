//! Workspace aggregate: documents, panes, and their focus invariants.

use std::collections::{BTreeMap, BTreeSet};

use iced::widget::pane_grid;

use crate::app::DocId;
use crate::document::Document;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneKind {
    Editor(DocId),
    Preview,
}

pub struct Workspace {
    pub(crate) documents: BTreeMap<DocId, Document>,
    pub(crate) active: DocId,
    pub(crate) next_id: DocId,
    pub(crate) panes: pane_grid::State<PaneKind>,
    pub(crate) focused: pane_grid::Pane,
    pub(crate) fullscreen: Option<pane_grid::Pane>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosePaneResult {
    NotClosed,
    Closed { preview: bool },
}

impl Workspace {
    pub fn new(first_id: DocId, document: Document) -> Self {
        let (panes, focused) = pane_grid::State::new(PaneKind::Editor(first_id));
        let mut documents = BTreeMap::new();
        documents.insert(first_id, document);
        Self {
            documents,
            active: first_id,
            next_id: first_id + 1,
            panes,
            focused,
            fullscreen: None,
        }
    }

    pub fn document(&self, id: DocId) -> Option<&Document> {
        self.documents.get(&id)
    }

    pub fn document_mut(&mut self, id: DocId) -> Option<&mut Document> {
        self.documents.get_mut(&id)
    }

    pub fn active_id(&self) -> DocId {
        self.active
    }

    pub fn active_document(&self) -> &Document {
        &self.documents[&self.active]
    }

    pub fn active_document_mut(&mut self) -> &mut Document {
        self.documents
            .get_mut(&self.active)
            .expect("active document exists")
    }

    pub fn pane_of_document(&self, id: DocId) -> Option<pane_grid::Pane> {
        self.panes
            .iter()
            .find(|(_, kind)| matches!(kind, PaneKind::Editor(doc) if *doc == id))
            .map(|(pane, _)| *pane)
    }

    pub fn editor_count(&self) -> usize {
        self.panes
            .iter()
            .filter(|(_, kind)| matches!(kind, PaneKind::Editor(_)))
            .count()
    }

    pub fn focus(&mut self, pane: pane_grid::Pane) {
        self.focused = pane;
        self.fullscreen = self.fullscreen.map(|_| pane);
        if let Some(PaneKind::Editor(id)) = self.panes.get(pane) {
            self.active = *id;
        }
    }

    pub fn allocate_document_id(&mut self) -> DocId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn insert_document(&mut self, document: Document) -> DocId {
        let id = self.allocate_document_id();
        self.documents.insert(id, document);
        id
    }

    pub fn split_editor(&mut self, id: DocId) {
        let anchor = self
            .pane_of_document(self.active)
            .unwrap_or(self.focused);
        match self
            .panes
            .split(pane_grid::Axis::Horizontal, anchor, PaneKind::Editor(id))
        {
            Some((pane, _)) => self.focus(pane),
            None => self.active = id,
        }
    }

    pub fn close_pane(&mut self, pane: pane_grid::Pane) -> ClosePaneResult {
        if matches!(self.panes.get(pane), Some(PaneKind::Editor(_))) && self.editor_count() <= 1 {
            return ClosePaneResult::NotClosed;
        }
        let Some((kind, sibling)) = self.panes.close(pane) else {
            return ClosePaneResult::NotClosed;
        };
        if self.focused == pane {
            self.focus(sibling);
        }
        self.validate();
        ClosePaneResult::Closed {
            preview: kind == PaneKind::Preview,
        }
    }

    pub fn cycle_focus(&mut self, direction: isize) -> Option<PaneKind> {
        let order: Vec<pane_grid::Pane> = self.panes.iter().map(|(pane, _)| *pane).collect();
        let index = order.iter().position(|pane| *pane == self.focused)?;
        let next = order[(index as isize + direction).rem_euclid(order.len() as isize) as usize];
        self.focus(next);
        self.panes.get(next).copied()
    }

    pub fn validate(&mut self) {
        let live: BTreeSet<DocId> = self
            .panes
            .iter()
            .filter_map(|(_, kind)| match kind {
                PaneKind::Editor(id) => Some(*id),
                PaneKind::Preview => None,
            })
            .collect();
        self.documents.retain(|id, _| live.contains(id));
        if !live.contains(&self.active) {
            self.active = *live.iter().next().expect("an editor remains");
        }
        if self.panes.get(self.focused).is_none() {
            self.focused = self
                .pane_of_document(self.active)
                .or_else(|| self.panes.iter().next().map(|(pane, _)| *pane))
                .expect("a pane remains");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_updates_the_active_document_and_fullscreen_target() {
        let mut workspace = Workspace::new(0, Document::untitled());
        let (second, _) = workspace
            .panes
            .split(pane_grid::Axis::Vertical, workspace.focused, PaneKind::Editor(1))
            .unwrap();
        workspace.documents.insert(1, Document::untitled());
        workspace.fullscreen = Some(workspace.focused);

        workspace.focus(second);
        assert_eq!(workspace.active, 1);
        assert_eq!(workspace.focused, second);
        assert_eq!(workspace.fullscreen, Some(second));
    }

    #[test]
    fn closing_a_pane_preserves_at_least_one_editor_and_drops_its_document() {
        let mut workspace = Workspace::new(0, Document::untitled());
        assert_eq!(workspace.close_pane(workspace.focused), ClosePaneResult::NotClosed);

        let id = workspace.insert_document(Document::untitled());
        workspace.split_editor(id);
        let pane = workspace.pane_of_document(id).unwrap();
        assert_eq!(
            workspace.close_pane(pane),
            ClosePaneResult::Closed { preview: false }
        );
        assert!(workspace.document(id).is_none());
        assert_eq!(workspace.editor_count(), 1);
    }
}
