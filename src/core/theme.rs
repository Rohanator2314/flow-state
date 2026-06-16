//! Theme loading in halloy's surface-oriented format.
//!
//! flow-state uses the same theme file format as
//! [halloy](https://github.com/squidowl/halloy) (GPL-3.0), so the community
//! theme library at <https://themes.halloy.chat> drops straight into
//! `~/.config/flow-state/themes/`. The format groups colors by UI *surface*
//! (`[general]`, `[text]`, `[buffer]`, …) rather than by syntax token, which
//! maps cleanly onto a GUI's chrome.
//!
//! The format structs ([`Styles`] and friends) plus the hex/color (de)serde
//! below are adapted from halloy's `data/src/appearance/theme.rs`. We parse
//! the subset of the schema a writing app needs (the IRC-specific keys —
//! nicknames, server messages, the base64 share format — are simply ignored
//! by serde) and resolve it down to the handful of [`Theme`] surfaces the
//! views consume. The bundled `Ferra` theme is halloy's default, used when no
//! theme is configured.

use std::path::Path;

use iced::theme::Palette;
use iced::Color;
use serde::{Deserialize, Deserializer};

/// halloy's default theme, bundled so the app has colors with zero config.
const FERRA: &str = include_str!("../../assets/themes/ferra.toml");

/// Resolved colors flow-state's widgets actually use, mapped from a
/// [`Styles`]. Keeping this small, app-facing type means the views never see
/// the full theme schema.
#[derive(Debug, Clone)]
pub struct Theme {
    /// Editor / writing canvas background (`buffer.background`).
    pub background: Color,
    /// Main text, used for the active paragraph (`text.primary`).
    pub text: Color,
    /// Dimmed paragraphs, hints, inactive chrome (`text.secondary`).
    pub text_inactive: Color,
    /// Chrome background: sidebar, pane title bars, status bar
    /// (`general.background`).
    pub surface: Color,
    /// Muted text on chrome (`text.secondary`).
    pub surface_text: Color,
    /// Accent for focus highlights and the active sidebar entry
    /// (`general.unread_indicator`).
    pub accent: Color,
    /// Pane and card borders (`general.border`).
    pub border: Color,
    /// Status / success color (`text.success`).
    pub success: Color,
    /// Warning color (`text.warning`).
    pub warning: Color,
    /// Error / danger color (`text.error`).
    pub danger: Color,
}

/// Hardcoded neutral dark theme — the infallible fallback used for any surface
/// a loaded theme leaves unset, so a partial theme file never renders blank.
fn fallback() -> Theme {
    Theme {
        background: rgb(0x1e1e1e),
        text: rgb(0xd4d4d4),
        text_inactive: rgb(0x6b6b6b),
        surface: rgb(0x161616),
        surface_text: rgb(0xa0a0a0),
        accent: rgb(0x569cd6),
        border: rgb(0x333333),
        success: rgb(0x6a9955),
        warning: rgb(0xd7ba7d),
        danger: rgb(0xf48771),
    }
}

impl Default for Theme {
    /// The bundled Ferra theme (halloy's default). Falls back to the neutral
    /// dark theme only if the bundled file somehow fails to parse.
    fn default() -> Self {
        Styles::ferra().to_theme()
    }
}

fn rgb(hex: u32) -> Color {
    Color::from_rgb8(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}

impl Theme {
    /// Load a halloy-format theme file and resolve it to flow-state surfaces.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        let styles: Styles = toml::from_str(&raw)?;
        Ok(styles.to_theme())
    }

    /// Build the matching `iced::Theme` so stock widgets follow the colors.
    pub fn iced_theme(&self) -> iced::Theme {
        iced::Theme::custom(
            "flow-state".to_string(),
            Palette {
                background: self.background,
                text: self.text,
                primary: self.accent,
                success: self.success,
                warning: self.warning,
                danger: self.danger,
            },
        )
    }
}

// ---------------------------------------------------------------------------
// halloy theme format (adapted subset)
// ---------------------------------------------------------------------------

/// The top-level theme file: surface groups. Unknown groups and keys
/// (`buttons`, `formatting`, nicknames, server messages, …) are ignored by
/// serde, so any halloy theme parses.
#[derive(Debug, Default, Deserialize)]
struct Styles {
    #[serde(default)]
    general: General,
    #[serde(default)]
    text: Text,
    #[serde(default)]
    buffer: Buffer,
}

#[derive(Debug, Default, Deserialize)]
struct General {
    #[serde(default)]
    background: Option<Hex>,
    #[serde(default)]
    border: Option<Hex>,
    #[serde(default)]
    unread_indicator: Option<Hex>,
}

#[derive(Debug, Default, Deserialize)]
struct Text {
    #[serde(default)]
    primary: Option<Hex>,
    #[serde(default)]
    secondary: Option<Hex>,
    #[serde(default)]
    tertiary: Option<Hex>,
    #[serde(default)]
    success: Option<Hex>,
    #[serde(default)]
    error: Option<Hex>,
    #[serde(default)]
    warning: Option<Hex>,
}

#[derive(Debug, Default, Deserialize)]
struct Buffer {
    #[serde(default)]
    background: Option<Hex>,
    #[serde(default)]
    border_selected: Option<Hex>,
}

impl Styles {
    /// The bundled Ferra theme, parsed. Bundled and unit-tested, so the
    /// `expect` is effectively a build-time guarantee.
    fn ferra() -> Self {
        toml::from_str(FERRA).expect("bundled ferra.toml parses")
    }

    /// Resolve the schema down to flow-state's surfaces, filling any unset
    /// color from the neutral [`fallback`].
    fn to_theme(&self) -> Theme {
        let fb = fallback();
        let c = |opt: Option<Hex>, default: Color| opt.map_or(default, |h| h.0);
        Theme {
            background: c(self.buffer.background, fb.background),
            text: c(self.text.primary, fb.text),
            text_inactive: c(self.text.secondary, fb.text_inactive),
            surface: c(self.general.background, fb.surface),
            surface_text: c(self.text.secondary, fb.surface_text),
            // Accent prefers the dedicated indicator, then the selected-border
            // color, then a bright text tone.
            accent: c(
                self.general.unread_indicator,
                c(self.buffer.border_selected, c(self.text.tertiary, fb.accent)),
            ),
            border: c(self.general.border, fb.border),
            success: c(self.text.success, fb.success),
            warning: c(self.text.warning, fb.warning),
            danger: c(self.text.error, fb.danger),
        }
    }
}

/// A theme color: either a plain `"#rrggbb[aa]"` string or a
/// `{ color = "#…", font_style = "…" }` table (halloy's `TextStyle` form). We
/// keep only the color; `font_style` is accepted and discarded.
#[derive(Debug, Clone, Copy)]
struct Hex(Color);

impl<'de> Deserialize<'de> for Hex {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Form {
            Plain(String),
            Table { color: String },
        }
        let hex = match Form::deserialize(deserializer)? {
            Form::Plain(s) | Form::Table { color: s } => s,
        };
        hex_to_color(&hex)
            .map(Hex)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid hex color: {hex}")))
    }
}

/// Parse `#rrggbb` or `#rrggbbaa` (halloy's `hex_to_color`).
fn hex_to_color(hex: &str) -> Option<Color> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 && hex.len() != 8 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    let a = if hex.len() == 8 {
        u8::from_str_radix(&hex[6..8], 16).ok()?
    } else {
        255
    };
    Some(Color::from_rgba8(r, g, b, f32::from(a) / 255.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_ferra_is_the_default() {
        // The default theme is halloy's Ferra, fully resolved (no fallbacks).
        let theme = Theme::default();
        assert_eq!(theme.background, rgb(0x242226)); // buffer.background
        assert_eq!(theme.text, rgb(0xfecdb2)); // text.primary
        assert_eq!(theme.text_inactive, rgb(0xab8a79)); // text.secondary
        assert_eq!(theme.surface, rgb(0x2b292d)); // general.background
        assert_eq!(theme.accent, rgb(0xffa07a)); // general.unread_indicator
        assert_eq!(theme.border, rgb(0x4f474d)); // general.border
    }

    #[test]
    fn loads_a_halloy_format_theme_file() {
        // Real halloy-format theme checked into the repo root.
        let theme = Theme::load(Path::new("catppuccin_mocha.toml")).unwrap();
        assert_eq!(theme.background, rgb(0x181825)); // buffer.background (mantle)
        assert_eq!(theme.text, rgb(0xcdd6f4)); // text.primary
        assert_eq!(theme.text_inactive, rgb(0xa6adc8)); // text.secondary
        assert_eq!(theme.surface, rgb(0x1e1e2e)); // general.background (base)
        assert_eq!(theme.accent, rgb(0xcba6f7)); // general.unread_indicator (mauve)
        assert_eq!(theme.success, rgb(0xa6e3a1)); // text.success (green)
    }

    #[test]
    fn hex_parses_rgb_and_rgba() {
        assert_eq!(hex_to_color("#010203"), Some(Color::from_rgb8(1, 2, 3)));
        assert_eq!(
            hex_to_color("#01020380"),
            Some(Color::from_rgba8(1, 2, 3, 128.0 / 255.0))
        );
        assert_eq!(hex_to_color("010203"), None); // no leading '#'
        assert_eq!(hex_to_color("#xyz"), None);
    }

    #[test]
    fn missing_keys_fall_back_without_blanking() {
        // A theme with only a background set keeps neutral defaults elsewhere.
        let styles: Styles =
            toml::from_str("[buffer]\nbackground = \"#123456\"\n").unwrap();
        let theme = styles.to_theme();
        assert_eq!(theme.background, rgb(0x123456));
        assert_eq!(theme.text, fallback().text);
        assert_eq!(theme.accent, fallback().accent);
    }
}
