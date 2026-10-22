use ratatui::style::{Modifier, Style};

use crate::ui::palette::{Palette, mix};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    palette: Palette,
}

impl Default for Theme {
    fn default() -> Self {
        Self::legacy()
    }
}

impl Theme {
    pub const fn from_palette(palette: Palette) -> Self {
        Self { palette }
    }

    pub const fn legacy() -> Self {
        Self::from_palette(Palette::legacy())
    }

    pub fn resolve(name: &str) -> Self {
        Palette::lookup(name)
            .map(Self::from_palette)
            .unwrap_or_else(Self::legacy)
    }

    pub fn accent(&self) -> Style {
        Style::default()
            .fg(self.palette.accent)
            .add_modifier(Modifier::BOLD)
    }

    pub fn normal(&self) -> Style {
        Style::default().fg(self.palette.foreground)
    }

    pub fn muted(&self) -> Style {
        Style::default().fg(self.palette.ansi[8])
    }

    pub fn warning(&self) -> Style {
        Style::default()
            .fg(self.palette.ansi[3])
            .add_modifier(Modifier::BOLD)
    }

    pub fn log_source(&self) -> Style {
        Style::default().fg(self.palette.accent)
    }

    pub fn app_log_source(&self) -> Style {
        Style::default().fg(self.palette.ansi[2])
    }

    pub fn error(&self) -> Style {
        Style::default()
            .fg(self.palette.ansi[1])
            .add_modifier(Modifier::BOLD)
    }

    pub fn success(&self) -> Style {
        Style::default()
            .fg(self.palette.ansi[2])
            .add_modifier(Modifier::BOLD)
    }

    pub fn status_bar(&self) -> Style {
        Style::default().fg(self.palette.foreground).bg(mix(
            self.palette.foreground,
            self.palette.background,
            0.92,
        ))
    }

    pub fn connected_badge(&self) -> Style {
        Style::default()
            .fg(self.palette.background)
            .bg(self.palette.ansi[2])
            .add_modifier(Modifier::BOLD)
    }

    pub fn connecting_badge(&self) -> Style {
        Style::default()
            .fg(self.palette.background)
            .bg(self.palette.ansi[3])
            .add_modifier(Modifier::BOLD)
    }

    pub fn offline_badge(&self) -> Style {
        Style::default()
            .fg(self.palette.foreground)
            .bg(mix(self.palette.foreground, self.palette.background, 0.75))
            .add_modifier(Modifier::BOLD)
    }

    pub fn ruleset_fresh_badge(&self) -> Style {
        self.connected_badge()
    }

    pub fn ruleset_stale_badge(&self) -> Style {
        Style::default()
            .fg(self.palette.background)
            .bg(self.palette.ansi[1])
            .add_modifier(Modifier::BOLD)
    }

    pub fn ruleset_updating_badge(&self) -> Style {
        self.connecting_badge()
    }

    pub fn ruleset_manual_badge(&self) -> Style {
        self.offline_badge()
    }

    pub fn toast_info(&self) -> Style {
        Style::default().fg(self.palette.accent)
    }

    pub fn toast_error(&self) -> Style {
        self.error()
    }

    pub fn border(&self) -> Style {
        Style::default().fg(self.palette.ansi[8])
    }

    pub fn selected(&self) -> Style {
        Style::default()
            .bg(self.palette.selection_background)
            .fg(self.palette.selection_foreground)
            .add_modifier(Modifier::BOLD)
    }

    pub fn selected_connected(&self) -> Style {
        Style::default()
            .bg(mix(self.palette.ansi[2], self.palette.background, 0.75))
            .fg(self.palette.ansi[2])
            .add_modifier(Modifier::BOLD)
    }

    pub fn popup_bg(&self) -> Style {
        Style::default().bg(self.palette.background)
    }

    pub fn background(&self) -> Style {
        Style::default().bg(self.palette.background)
    }

    pub fn palette_background(&self) -> ratatui::style::Color {
        self.palette.background
    }

    pub fn palette_foreground(&self) -> ratatui::style::Color {
        self.palette.foreground
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn legacy_accent_is_cyan_bold() {
        let s = Theme::legacy().accent();
        assert_eq!(s.fg, Some(Color::Cyan));
        assert!(s.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn legacy_normal_is_gray() {
        let s = Theme::legacy().normal();
        assert_eq!(s.fg, Some(Color::Gray));
    }

    #[test]
    fn log_sources_use_distinct_colors() {
        let theme = Theme::legacy();
        assert_eq!(theme.app_log_source().fg, Some(Color::Green));
        assert_eq!(theme.log_source().fg, Some(Color::Cyan));
        assert_ne!(theme.app_log_source().fg, theme.log_source().fg);
    }

    #[test]
    fn legacy_error_is_red_bold() {
        let s = Theme::legacy().error();
        assert_eq!(s.fg, Some(Color::Red));
        assert!(s.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn legacy_success_is_green_bold() {
        let s = Theme::legacy().success();
        assert_eq!(s.fg, Some(Color::Green));
        assert!(s.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn ruleset_badges_use_semantic_backgrounds() {
        let theme = Theme::legacy();
        assert_eq!(theme.ruleset_fresh_badge(), theme.connected_badge());
        assert_eq!(theme.ruleset_updating_badge(), theme.connecting_badge());
        assert_eq!(theme.ruleset_manual_badge(), theme.offline_badge());
        assert_eq!(theme.ruleset_stale_badge().bg, Some(Color::Red));
        assert!(
            theme
                .ruleset_stale_badge()
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn legacy_border_is_dark_gray() {
        let s = Theme::legacy().border();
        assert_eq!(s.fg, Some(Color::DarkGray));
    }

    #[test]
    fn legacy_popup_bg_is_black() {
        let s = Theme::legacy().popup_bg();
        assert_eq!(s.bg, Some(Color::Black));
    }

    #[test]
    fn legacy_selected_matches_pre_theming_rgb() {
        let s = Theme::legacy().selected();
        assert_eq!(s.bg, Some(Color::Rgb(45, 45, 85)));
        assert_eq!(s.fg, Some(Color::Cyan));
    }

    #[test]
    fn default_theme_equals_legacy() {
        assert_eq!(Theme::default(), Theme::legacy());
    }

    #[test]
    fn resolve_known_theme_returns_omarchy_palette() {
        let t = Theme::resolve("tokyo-night");
        assert_eq!(t.accent().fg, Some(Color::Rgb(0x7a, 0xa2, 0xf7)));
    }

    #[test]
    fn resolve_unknown_theme_falls_back_to_legacy() {
        let t = Theme::resolve("not-a-real-theme");
        assert_eq!(t, Theme::legacy());
    }

    #[test]
    fn exposes_raw_palette_foreground() {
        let theme = Theme::resolve("catppuccin-latte");
        assert_eq!(theme.palette_foreground(), Color::Rgb(0x4c, 0x4f, 0x69));
    }
}
