use crate::app::effect::Effect;
use crate::app::model::{Model, Overlay};
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::update::key::settings_menu::{finish_settings_overlay, return_to_settings_menu};
use crate::app::update::status::push_status;

/// Build the picker list. First entry is the Auto-follow-Omarchy slot
/// (only present when Omarchy is detected); the remaining entries are
/// the bundled palette names in alphabetical order.
pub fn theme_picker_slugs() -> Vec<String> {
    let mut out = Vec::new();
    if crate::omarchy::detect_omarchy_theme().is_some() {
        out.push(crate::tui_client::theme_watch::OMARCHY_SENTINEL.to_string());
    }
    out.extend(
        crate::ui::palette::Palette::bundled_names()
            .into_iter()
            .map(str::to_string),
    );
    out
}

/// Friendly label for an entry in the picker. The Auto entry is
/// annotated with the currently active Omarchy theme so the user can
/// see what would be applied; all others are returned verbatim.
fn theme_picker_label(slug: &str) -> String {
    if slug == crate::tui_client::theme_watch::OMARCHY_SENTINEL {
        match crate::omarchy::detect_omarchy_theme() {
            Some(name) => format!("Auto ({})", shorten_theme_name(&name)),
            None => "Auto (Omarchy)".to_string(),
        }
    } else {
        slug.to_string()
    }
}

const MAX_THEME_NAME_CHARS: usize = 12;

pub fn shorten_theme_name(name: &str) -> String {
    if name.chars().count() <= MAX_THEME_NAME_CHARS {
        return name.to_string();
    }
    let kept: String = name.chars().take(MAX_THEME_NAME_CHARS - 1).collect();
    format!("{kept}…")
}

pub fn theme_picker_labels() -> Vec<String> {
    theme_picker_slugs()
        .iter()
        .map(|s| theme_picker_label(s))
        .collect()
}

/// Handle keys inside the theme picker overlay. j/k drive a live preview
/// of the highlighted entry (the daemon stores the draft slug; the TUI
/// client resolves it into the actual `Theme` on snapshot apply).
pub(in crate::app::update) fn handle_theme_picker(model: &mut Model, key: KeyEvent) -> Vec<Effect> {
    let slugs = theme_picker_slugs();
    if slugs.is_empty() {
        model.settings_menu_return = None;
        model.overlay = Overlay::None;
        return vec![];
    }
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            crate::ui::nav::select_next(&mut model.theme_selected, slugs.len());
            model.theme_draft = slugs.get(model.theme_selected).cloned();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            crate::ui::nav::select_prev(&mut model.theme_selected);
            model.theme_draft = slugs.get(model.theme_selected).cloned();
        }
        KeyCode::Char('G') => {
            crate::ui::nav::select_last(&mut model.theme_selected, slugs.len());
            model.theme_draft = slugs.get(model.theme_selected).cloned();
        }
        KeyCode::Enter => {
            let Some(slug) = slugs.get(model.theme_selected).cloned() else {
                return vec![];
            };
            let changed = model.config.settings.theme != slug;
            model.config.settings.theme = slug.clone();
            model.theme_draft = None;
            finish_settings_overlay(model);
            if changed {
                let mut effects = vec![Effect::SaveConfig];
                push_status(
                    &mut effects,
                    model,
                    crate::app::model::AppStatus::Info(format!("Theme: {}", slug)),
                );
                return effects;
            }
            return vec![];
        }
        KeyCode::Char('q') | KeyCode::Esc => {
            model.theme_draft = None;
            model.settings_menu_return = None;
            model.overlay = Overlay::None;
        }
        KeyCode::Backspace if model.settings_menu_return.is_some() => {
            model.theme_draft = None;
            return_to_settings_menu(model);
        }
        _ => {}
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::SettingsMenuPage;
    use crate::app::msg::Msg;
    use crate::app::update::key::handle_key;
    use crate::app::update::update;
    use crate::test_helpers::*;

    /// `t` opens the theme picker overlay and positions the cursor on the
    /// currently committed theme slug.
    #[test]
    fn t_key_opens_theme_picker_at_current_theme() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let mut model = model_with_profiles(vec![]);
        model.config.settings.theme = "gruvbox".into();
        let key = KeyEvent::new(KeyCode::Char('C'), crossterm::event::KeyModifiers::NONE);
        let _ = update(&mut model, Msg::Key(key));
        assert_eq!(model.overlay, Overlay::ThemeSettings);
        let slugs = crate::app::update::theme_picker_slugs();
        assert_eq!(
            slugs.get(model.theme_selected).map(String::as_str),
            Some("gruvbox")
        );
        assert!(model.theme_draft.is_none(), "draft starts cleared on open");
    }

    /// j/k inside the picker update both the cursor and the draft slug —
    /// the TUI client maps draft → live `model.theme` on snapshot apply.
    #[test]
    fn theme_picker_j_k_set_draft() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let mut model = model_with_profiles(vec![]);
        model.config.settings.theme = "tokyo-night".into();
        let _ = update(
            &mut model,
            Msg::Key(KeyEvent::new(
                KeyCode::Char('C'),
                crossterm::event::KeyModifiers::NONE,
            )),
        );
        let before = model.theme_selected;
        let _ = update(
            &mut model,
            Msg::Key(KeyEvent::new(
                KeyCode::Char('j'),
                crossterm::event::KeyModifiers::NONE,
            )),
        );
        assert_eq!(model.theme_selected, before + 1);
        let slugs = crate::app::update::theme_picker_slugs();
        assert_eq!(
            model.theme_draft.as_deref(),
            slugs.get(model.theme_selected).map(String::as_str)
        );
    }

    /// Enter persists the draft into `settings.theme`, clears the draft,
    /// closes the overlay, and emits SaveConfig (only when changed).
    #[test]
    fn theme_picker_enter_commits_and_saves() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let mut model = model_with_profiles(vec![]);
        model.config.settings.theme = "tokyo-night".into();
        let _ = update(
            &mut model,
            Msg::Key(KeyEvent::new(
                KeyCode::Char('C'),
                crossterm::event::KeyModifiers::NONE,
            )),
        );
        // Move cursor to a known slug.
        let slugs = crate::app::update::theme_picker_slugs();
        let target_idx = slugs
            .iter()
            .position(|s| s == "nord")
            .expect("nord present");
        model.theme_selected = target_idx;
        let effects = update(
            &mut model,
            Msg::Key(KeyEvent::new(
                KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE,
            )),
        );
        assert_eq!(model.config.settings.theme, "nord");
        assert!(model.theme_draft.is_none());
        assert_eq!(model.overlay, Overlay::None);
        assert!(
            effects.iter().any(|e| matches!(e, Effect::SaveConfig)),
            "Enter must request SaveConfig"
        );
    }

    /// Esc discards the draft and closes the overlay without touching
    /// `settings.theme`.
    #[test]
    fn theme_picker_esc_discards_draft() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let mut model = model_with_profiles(vec![]);
        model.config.settings.theme = "tokyo-night".into();
        let _ = update(
            &mut model,
            Msg::Key(KeyEvent::new(
                KeyCode::Char('C'),
                crossterm::event::KeyModifiers::NONE,
            )),
        );
        let _ = update(
            &mut model,
            Msg::Key(KeyEvent::new(
                KeyCode::Char('j'),
                crossterm::event::KeyModifiers::NONE,
            )),
        );
        assert!(model.theme_draft.is_some());
        let _ = update(
            &mut model,
            Msg::Key(KeyEvent::new(
                KeyCode::Esc,
                crossterm::event::KeyModifiers::NONE,
            )),
        );
        assert_eq!(model.overlay, Overlay::None);
        assert!(model.theme_draft.is_none());
        assert_eq!(model.config.settings.theme, "tokyo-night");
    }

    /// On a non-Omarchy system the picker omits the Auto entry, so the
    /// list is exactly the 22 bundled palette names.
    #[test]
    fn theme_picker_omits_auto_when_omarchy_absent() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        unsafe { std::env::set_var("XDG_STATE_HOME", state.path()) };
        let slugs = crate::app::update::theme_picker_slugs();
        assert!(!slugs.iter().any(|s| s == "omarchy"));
        assert_eq!(slugs.len(), 22);
    }

    #[test]
    fn long_theme_names_are_shortened_to_twelve_chars() {
        assert_eq!(shorten_theme_name("catppuccin-latte"), "catppuccin-…");
        assert_eq!(shorten_theme_name("last-horizon"), "last-horizon");
        assert_eq!(shorten_theme_name("nord"), "nord");
        assert_eq!(
            shorten_theme_name("theme-name-that-keeps-going-past-the-overlay"),
            "theme-name-…"
        );
    }

    #[test]
    fn theme_list_opens_only_from_main_screen_shortcut() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);
        model.config.settings.theme = "gruvbox".into();

        assert!(handle_key(&mut model, key('t')).is_empty());
        assert_eq!(model.overlay, Overlay::SettingsMenu(SettingsMenuPage::Root));

        model.overlay = Overlay::None;
        handle_key(&mut model, key('C'));

        assert_eq!(model.overlay, Overlay::ThemeSettings);
        assert_eq!(
            theme_picker_slugs()
                .get(model.theme_selected)
                .map(String::as_str),
            Some("gruvbox")
        );
        assert_eq!(model.theme_draft, None);
        assert_eq!(model.settings_menu_return, None);
    }
}
