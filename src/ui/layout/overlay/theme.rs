use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::model::Model;

use super::popup::draw_selection_modal;
use super::settings_overlay_footer;

/// Draw the theme picker overlay. Lists all bundled palettes plus an
/// optional Auto entry (when Omarchy is detected) that maps to the
/// `"omarchy"` sentinel slug. The active row is the one matching the
/// committed `Settings.theme`; the cursor tracks `model.theme_selected`.
pub(super) fn draw(frame: &mut Frame, model: &Model, area: Rect) {
    let slugs = crate::app::update::theme_picker_slugs();
    let labels: Vec<String> = crate::app::update::theme_picker_labels();
    let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let active = slugs.iter().position(|s| s == &model.config.settings.theme);
    draw_selection_modal(
        frame,
        &model.theme,
        area,
        "Settings › Theme",
        &label_refs,
        model.theme_selected,
        active,
        settings_overlay_footer(model),
    );
}

#[cfg(test)]
mod tests {

    use crate::app::model::Overlay;
    use crate::test_helpers::{
        APP_WINDOW_COLS, APP_WINDOW_ROWS, model_with_profiles, snapshot_terminal,
    };
    use crate::ui::layout::{MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH};

    /// Theme picker overlay rendered with the dark default palette.
    /// Pins the layout, label format, and active-row highlighting.
    #[test]
    fn draw_theme_settings_overlay_snapshot() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _config_home = crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", dir.path());
        let _state_home = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", dir.path());
        let mut model = model_with_profiles(vec![]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.overlay = Overlay::ThemeSettings;
        model.config.settings.theme = "gruvbox".into();
        let slugs = crate::app::update::theme_picker_slugs();
        model.theme_selected = slugs
            .iter()
            .position(|s| s == &model.config.settings.theme)
            .unwrap_or(0);
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    #[test]
    fn draw_theme_settings_overlay_at_minimum_terminal_size_snapshot() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _config_home = crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", dir.path());
        let _state_home = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", dir.path());
        let mut model = model_with_profiles(vec![]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.overlay = Overlay::ThemeSettings;
        model.config.settings.theme = "gruvbox".into();
        let slugs = crate::app::update::theme_picker_slugs();
        model.theme_selected = slugs
            .iter()
            .position(|s| s == &model.config.settings.theme)
            .unwrap_or(0);
        insta::assert_snapshot!(snapshot_terminal(
            &model,
            MIN_TERMINAL_WIDTH,
            MIN_TERMINAL_HEIGHT
        ));
    }

    /// Theme picker rendered with a light palette — sanity check for
    /// contrast on backgrounds where the dark-mode defaults don't apply.
    #[test]
    fn draw_theme_settings_overlay_light_snapshot() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("omarchy/current");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(
            current.join("theme.name"),
            "theme-name-that-keeps-going-past-the-overlay",
        )
        .unwrap();
        let _config_home = crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", dir.path());
        let _state_home = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", dir.path());
        let mut model = model_with_profiles(vec![]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.overlay = Overlay::ThemeSettings;
        model.config.settings.theme = "catppuccin-latte".into();
        model.theme = crate::ui::styles::Theme::resolve(&model.config.settings.theme);
        let slugs = crate::app::update::theme_picker_slugs();
        model.theme_selected = slugs
            .iter()
            .position(|s| s == &model.config.settings.theme)
            .unwrap_or(0);
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    #[test]
    fn theme_picker_scrolls_to_keep_selection_and_footer_visible() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _config_home = crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", dir.path());
        let _state_home = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", dir.path());
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::ThemeSettings;
        model.theme_selected = crate::app::update::theme_picker_slugs().len() - 1;

        let rendered = snapshot_terminal(&model, MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT);
        assert!(rendered.contains("white"));
        assert!(rendered.contains("󰌑 apply · q/󱊷 close"));
        assert!(!rendered.contains("j/k navigate"));
        assert!(!rendered.contains("catppuccin-latte"));
    }
}
