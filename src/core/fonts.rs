//! Enumerating the system's installed font families for the font switcher.
//!
//! iced's text backend (cosmic-text) resolves a font by family name through
//! the same `fontdb` we query here, so every name this returns is one
//! `iced::Font::with_name` can actually load. The scan is done once and cached
//! — loading system fonts touches the filesystem and is not free.

use std::sync::OnceLock;

static FONTS: OnceLock<Vec<String>> = OnceLock::new();

/// Installed font families, de-duplicated and sorted. Cached after the first
/// call.
pub fn available() -> &'static [String] {
    FONTS.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();

        let mut names: Vec<String> = db
            .faces()
            .filter_map(|face| face.families.first().map(|(name, _)| name.clone()))
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    })
}
