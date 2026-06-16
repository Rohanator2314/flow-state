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
    /// Name of a theme file (without `.toml`) in the themes directory;
    /// empty = built-in theme.
    pub theme: String,
    pub latex_compiler: String,
    /// Initial fraction of the pane area given to the editor.
    pub preview_split_ratio: f32,
    /// Dim the paragraphs outside the one being written (the focus effect).
    pub focus_dimming: bool,
    /// Editor font family name; empty = the built-in sans-serif default.
    pub editor_font: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: String::new(),
            latex_compiler: "pdflatex".to_string(),
            preview_split_ratio: 0.5,
            focus_dimming: true,
            editor_font: String::new(),
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

    /// Resolve the configured theme from `~/.config/flow-state/themes/`,
    /// falling back to the built-in theme.
    pub fn load_theme(&self) -> (Theme, Option<String>) {
        if self.theme.is_empty() {
            return (Theme::default(), None);
        }
        let Some(dir) = config_dir() else {
            return (Theme::default(), None);
        };
        let path = dir.join("themes").join(format!("{}.toml", self.theme));
        match Theme::load(&path) {
            Ok(theme) => (theme, None),
            Err(e) => (
                Theme::default(),
                Some(format!("theme '{}' not loaded ({e}); using built-in", self.theme)),
            ),
        }
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

/// Theme names available to the in-app switcher: the built-in default plus
/// every `*.toml` in the themes directory.
pub const BUILTIN_THEME: &str = "(default)";

pub fn available_themes() -> Vec<String> {
    let mut names = vec![BUILTIN_THEME.to_string()];
    if let Some(dir) = config_dir().map(|d| d.join("themes"))
        && let Ok(read) = std::fs::read_dir(dir)
    {
        let mut found: Vec<String> = read
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                (path.extension()? == "toml")
                    .then(|| path.file_stem()?.to_str().map(str::to_string))?
            })
            .collect();
        found.sort();
        names.extend(found);
    }
    names
}
