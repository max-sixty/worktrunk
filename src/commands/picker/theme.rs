//! Theme resolution for the `wt switch` picker.
//!
//! Most Worktrunk output uses basic ANSI colors and naturally follows the
//! terminal palette. The skim picker is different: selected-row colors are
//! configured through skim's own `--color` string, so hard-coded 256-color
//! values can clash with desktop-managed terminal themes.

use std::path::Path;

use serde::Deserialize;

const FALLBACK_PICKER_COLORS: &str =
    "fg:-1,bg:-1,header:-1,matched:108,current:237,current_bg:251,current_match:108";

#[derive(Debug, Deserialize, PartialEq)]
struct OmarchyColors {
    cursor: String,
    foreground: String,
    background: String,
    color0: String,
    color1: String,
    color2: String,
    color4: String,
    color5: String,
    color6: String,
    color8: String,
}

impl OmarchyColors {
    fn from_path(path: &Path) -> Option<Self> {
        let content = std::fs::read_to_string(path).ok()?;
        let colors: Self = toml::from_str(&content).ok()?;
        colors.is_valid().then_some(colors)
    }

    fn is_valid(&self) -> bool {
        [
            &self.cursor,
            &self.foreground,
            &self.background,
            &self.color0,
            &self.color1,
            &self.color2,
            &self.color4,
            &self.color5,
            &self.color6,
            &self.color8,
        ]
        .into_iter()
        .all(|color| is_hex_color(color))
    }

    // Keep this mapping aligned with Omarchy's generated Skim palette in fzf.sh.tpl.
    fn to_skim_color_scheme(&self) -> String {
        format!(
            "fg:{fg},bg:{bg},matched:{color0},matched_bg:{cursor},current:{fg},\
current_bg:{color8},current_match:{bg},current_match_bg:{cursor},spinner:{color2},\
info:{color5},prompt:{color4},cursor:{color1},selected:{color1},header:{color6},border:{color8}",
            fg = self.foreground,
            bg = self.background,
            cursor = self.cursor,
            color0 = self.color0,
            color1 = self.color1,
            color2 = self.color2,
            color4 = self.color4,
            color5 = self.color5,
            color6 = self.color6,
            color8 = self.color8,
        )
    }
}

pub(super) fn picker_color_scheme() -> String {
    omarchy_colors_path()
        .as_deref()
        .and_then(OmarchyColors::from_path)
        .map(|colors| colors.to_skim_color_scheme())
        .unwrap_or_else(|| FALLBACK_PICKER_COLORS.to_string())
}

fn omarchy_colors_path() -> Option<std::path::PathBuf> {
    home::home_dir().map(|home| {
        home.join(".config")
            .join("omarchy")
            .join("current")
            .join("theme")
            .join("colors.toml")
    })
}

fn is_hex_color(value: &str) -> bool {
    let Some(hex) = value.strip_prefix('#') else {
        return false;
    };

    hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omarchy_colors_parse_active_theme_file() {
        let path = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            path.path(),
            r##"
accent = "#89b4fa"
cursor = "#f5e0dc"
foreground = "#cdd6f4"
background = "#1e1e2e"
color0 = "#45475a"
color1 = "#f38ba8"
color2 = "#a6e3a1"
color4 = "#89b4fa"
color5 = "#f5c2e7"
color6 = "#94e2d5"
color8 = "#585b70"
"##,
        )
        .unwrap();

        let colors = OmarchyColors::from_path(path.path()).unwrap();

        assert_eq!(
            colors.to_skim_color_scheme(),
            "fg:#cdd6f4,bg:#1e1e2e,matched:#45475a,matched_bg:#f5e0dc,\
current:#cdd6f4,current_bg:#585b70,current_match:#1e1e2e,\
current_match_bg:#f5e0dc,spinner:#a6e3a1,info:#f5c2e7,prompt:#89b4fa,\
cursor:#f38ba8,selected:#f38ba8,header:#94e2d5,border:#585b70"
        );
    }

    #[test]
    fn omarchy_colors_rejects_missing_or_invalid_colors() {
        let path = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            path.path(),
            r##"
accent = "#89b4fa"
cursor = "#f5e0dc"
foreground = "cdd6f4"
background = "#1e1e2e"
color0 = "#45475a"
color1 = "#f38ba8"
color2 = "#a6e3a1"
color4 = "#89b4fa"
color5 = "#f5c2e7"
color6 = "#94e2d5"
color8 = "#585b70"
"##,
        )
        .unwrap();

        assert_eq!(OmarchyColors::from_path(path.path()), None);
    }

    #[test]
    fn missing_omarchy_colors_falls_back_to_existing_picker_palette() {
        assert_eq!(
            OmarchyColors::from_path(Path::new("/path/that/does/not/exist")),
            None
        );
        assert_eq!(
            FALLBACK_PICKER_COLORS,
            "fg:-1,bg:-1,header:-1,matched:108,current:237,current_bg:251,current_match:108"
        );
    }
}
