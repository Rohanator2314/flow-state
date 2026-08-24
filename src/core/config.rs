//! User configuration: `~/.config/flow-state/config.toml`.
//!
//! Loading never fails the app — a missing file means defaults, an invalid
//! file means defaults plus a warning surfaced in the status bar (the
//! `(value, warning)` return shape). To add an option: add a field to
//! [`Config`] with a `Default` value; serde fills it in when absent.
//!
//! The in-app menu edits the config live and persists it via
//! [`Config::save`]. Note: saving re-serializes the file, so hand-written
//! comments in config.toml don't survive a save from the menu.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::core::theme::Theme;

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// Bundled theme name or a theme file stem from the user themes directory;
    /// empty = the built-in default.
    pub theme: String,
    pub latex_compiler: String,
    /// Initial fraction of the pane area given to the editor.
    pub preview_split_ratio: f32,
    /// Dim the paragraphs outside the one being written (the focus effect).
    pub focus_dimming: bool,
    /// Keep the active paragraph vertically centered (typewriter scrolling).
    pub typewriter_scroll: bool,
    /// Give the active paragraph a soft glow.
    pub paragraph_glow: bool,
    /// Editor font family name; empty = the built-in sans-serif default.
    pub editor_font: String,
    /// Check spelling locally using a Hunspell-format dictionary.
    pub spell_check: bool,
    /// Hunspell locale basename, such as `en_US`.
    pub spell_language: String,
    /// Optional dictionary basename, `.aff`/`.dic` file, or containing folder.
    /// Empty discovers the locale in application and system directories.
    pub spell_dictionary: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: String::new(),
            latex_compiler: "pdflatex".to_string(),
            preview_split_ratio: 0.5,
            focus_dimming: true,
            typewriter_scroll: false,
            paragraph_glow: false,
            editor_font: String::new(),
            spell_check: true,
            spell_language: "en_US".to_string(),
            spell_dictionary: String::new(),
        }
    }
}

pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("flow-state"))
}

impl Config {
    /// Load the config file, falling back to defaults.
    pub fn load() -> (Self, Option<String>) {
        let Some(path) = config_dir().map(|d| d.join("config.toml")) else {
            return (Self::default(), None);
        };
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(_) => return (Self::default(), None),
        };
        match toml::from_str::<Config>(&raw) {
            Ok(config) => (config, None),
            Err(e) => (
                Self::default(),
                Some(format!("config.toml invalid, using defaults: {e}")),
            ),
        }
    }

    /// Resolve the configured user or release theme.
    pub fn load_theme(&self) -> (Theme, Option<String>) {
        resolve_theme(&self.theme)
    }

    /// Editor share of the pane area, clamped to a sane range.
    pub fn split_ratio(&self) -> f32 {
        self.preview_split_ratio.clamp(0.2, 0.8)
    }

    /// Persist the config. Called by the in-app menu on every change.
    pub fn save(&self) -> Result<(), String> {
        let dir = config_dir().ok_or("no config directory")?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let toml = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("config.toml"), toml).map_err(|e| e.to_string())
    }
}

/// Theme names available to the in-app switcher: the default, every release
/// theme, and any user-installed `*.toml` theme.
pub const BUILTIN_THEME: &str = "(default)";

pub fn available_themes() -> Vec<String> {
    let mut available: Vec<String> = Theme::bundled_names().map(str::to_string).collect();
    if let Some(dir) = config_dir().map(|d| d.join("themes"))
        && let Ok(read) = std::fs::read_dir(dir)
    {
        available.extend(read
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                (path.extension()? == "toml")
                    .then(|| path.file_stem()?.to_str().map(str::to_string))?
            }));
    }
    available.sort();
    available.dedup();

    let mut names = vec![BUILTIN_THEME.to_string()];
    names.extend(available);
    names
}

/// User files override release themes with the same name. Missing user files
/// fall back to the compiled release bundle, then to the default theme.
pub fn resolve_theme(name: &str) -> (Theme, Option<String>) {
    if name.is_empty() || name == BUILTIN_THEME {
        return (Theme::default(), None);
    }

    if let Some(path) = config_dir().map(|dir| dir.join("themes").join(format!("{name}.toml")))
        && path.exists()
    {
        return match Theme::load(&path) {
            Ok(theme) => (theme, None),
            Err(error) => (
                Theme::default(),
                Some(format!("theme '{name}' not loaded ({error}); using built-in")),
            ),
        };
    }

    Theme::bundled(name).map_or_else(
        || {
            (
                Theme::default(),
                Some(format!("theme '{name}' not found; using built-in")),
            )
        },
        |theme| (theme, None),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_themes_are_available_without_user_configuration() {
        let options = available_themes();
        assert_eq!(options.first().map(String::as_str), Some(BUILTIN_THEME));
        for name in Theme::bundled_names() {
            assert!(options.iter().any(|option| option == name));
            assert!(resolve_theme(name).1.is_none());
        }
    }
}
