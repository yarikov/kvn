use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use notify::{RecursiveMode, Watcher};

use crate::app::msg::Msg;
use crate::omarchy::{detect_omarchy_theme, omarchy_current_dir, theme_colors_path};
use crate::ui::palette::Palette;
use crate::ui::styles::Theme;

pub const OMARCHY_SENTINEL: &str = "omarchy";

pub const DEFAULT_THEME: &str = "tokyo-night";

pub fn resolve_active(settings_theme: &str) -> Theme {
    if settings_theme == OMARCHY_SENTINEL {
        load_current_theme().unwrap_or_else(|| {
            detect_omarchy_theme()
                .as_deref()
                .and_then(Palette::lookup)
                .map(Theme::from_palette)
                .unwrap_or_else(|| Theme::resolve(DEFAULT_THEME))
        })
    } else {
        Theme::resolve(settings_theme)
    }
}

fn load_current_theme() -> Option<Theme> {
    let raw = std::fs::read_to_string(theme_colors_path()?).ok()?;
    Palette::from_omarchy_toml(&raw)
        .ok()
        .map(Theme::from_palette)
}

pub fn spawn_theme_watcher(tx: Sender<Msg>) {
    let Some(watch_dir) = omarchy_current_dir() else {
        return;
    };
    if !watch_dir.exists() {
        return;
    }

    thread::spawn(move || run_watcher(tx, watch_dir));
}

fn run_watcher(tx: Sender<Msg>, watch_dir: PathBuf) {
    let (notify_tx, notify_rx) = std::sync::mpsc::channel();
    let mut watcher = match notify::recommended_watcher(move |res| {
        let _ = notify_tx.send(res);
    }) {
        Ok(w) => w,
        Err(_) => return,
    };
    if watcher.watch(&watch_dir, RecursiveMode::Recursive).is_err() {
        return;
    }

    let mut last_theme = load_current_theme();

    loop {
        let Ok(_first) = notify_rx.recv() else {
            return;
        };
        while notify_rx.recv_timeout(Duration::from_millis(50)).is_ok() {}

        if let Some(current_theme) = load_current_theme()
            && Some(current_theme) != last_theme
        {
            if tx.send(Msg::ThemeChanged(current_theme)).is_err() {
                return;
            }
            last_theme = Some(current_theme);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_helpers::ENV_LOCK;

    #[test]
    fn resolve_active_uses_named_slug_directly() {
        let _guard = ENV_LOCK.lock().unwrap();
        let config = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", config.path()) };
        unsafe { std::env::set_var("XDG_STATE_HOME", state.path()) };
        let theme = resolve_active("gruvbox");
        assert_eq!(
            theme.accent().fg,
            Some(ratatui::style::Color::Rgb(0x7d, 0xae, 0xa3))
        );
    }

    #[test]
    fn resolve_active_omarchy_sentinel_without_file_falls_back_to_default() {
        let _guard = ENV_LOCK.lock().unwrap();
        let config = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", config.path()) };
        unsafe { std::env::set_var("XDG_STATE_HOME", state.path()) };
        let theme = resolve_active(OMARCHY_SENTINEL);
        assert_eq!(
            theme.accent().fg,
            Some(ratatui::style::Color::Rgb(0x7a, 0xa2, 0xf7))
        );
    }

    #[test]
    fn resolve_active_omarchy_sentinel_reads_theme_name_when_present() {
        let _guard = ENV_LOCK.lock().unwrap();
        let config = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", config.path()) };
        unsafe { std::env::set_var("XDG_STATE_HOME", state.path()) };
        let current = state.path().join("omarchy").join("current");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir_all(current.join("theme")).unwrap();
        std::fs::write(current.join("theme.name"), "custom-theme\n").unwrap();
        let raw = include_str!("../../themes/nord.toml").replace("#81a1c1", "#010203");
        std::fs::write(current.join("theme").join("colors.toml"), raw).unwrap();
        let theme = resolve_active(OMARCHY_SENTINEL);
        assert_eq!(theme.accent().fg, Some(ratatui::style::Color::Rgb(1, 2, 3)));
    }

    #[test]
    fn auto_prefers_runtime_colors_for_bundled_slug() {
        let _guard = ENV_LOCK.lock().unwrap();
        let config = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", config.path()) };
        unsafe { std::env::set_var("XDG_STATE_HOME", state.path()) };
        let current = state.path().join("omarchy").join("current");
        std::fs::create_dir_all(current.join("theme")).unwrap();
        std::fs::write(current.join("theme.name"), "nord\n").unwrap();
        let raw = include_str!("../../themes/nord.toml").replace("#81a1c1", "#112233");
        std::fs::write(current.join("theme").join("colors.toml"), raw).unwrap();

        assert_eq!(
            resolve_active(OMARCHY_SENTINEL).accent().fg,
            Some(ratatui::style::Color::Rgb(0x11, 0x22, 0x33))
        );
        assert_eq!(
            resolve_active("nord").accent().fg,
            Some(ratatui::style::Color::Rgb(0x81, 0xa1, 0xc1))
        );
    }

    #[test]
    fn auto_falls_back_to_bundled_slug_for_invalid_runtime_colors() {
        let _guard = ENV_LOCK.lock().unwrap();
        let config = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", config.path()) };
        unsafe { std::env::set_var("XDG_STATE_HOME", state.path()) };
        let current = state.path().join("omarchy").join("current");
        std::fs::create_dir_all(current.join("theme")).unwrap();
        std::fs::write(current.join("theme.name"), "nord\n").unwrap();
        std::fs::write(current.join("theme").join("colors.toml"), "invalid").unwrap();

        assert_eq!(
            resolve_active(OMARCHY_SENTINEL).accent().fg,
            Some(ratatui::style::Color::Rgb(0x81, 0xa1, 0xc1))
        );
    }

    #[test]
    fn auto_reloads_changed_colors_without_slug_change() {
        let _guard = ENV_LOCK.lock().unwrap();
        let config = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", config.path()) };
        unsafe { std::env::set_var("XDG_STATE_HOME", state.path()) };
        let current = state.path().join("omarchy").join("current");
        let colors_path = current.join("theme").join("colors.toml");
        std::fs::create_dir_all(current.join("theme")).unwrap();
        std::fs::write(current.join("theme.name"), "custom-theme\n").unwrap();
        let original = include_str!("../../themes/nord.toml").replace("#81a1c1", "#112233");
        std::fs::write(&colors_path, original).unwrap();
        assert_eq!(
            resolve_active(OMARCHY_SENTINEL).accent().fg,
            Some(ratatui::style::Color::Rgb(0x11, 0x22, 0x33))
        );

        let changed = include_str!("../../themes/nord.toml").replace("#81a1c1", "#445566");
        std::fs::write(colors_path, changed).unwrap();
        assert_eq!(
            resolve_active(OMARCHY_SENTINEL).accent().fg,
            Some(ratatui::style::Color::Rgb(0x44, 0x55, 0x66))
        );
    }

    #[test]
    fn auto_falls_back_to_default_for_unknown_invalid_theme() {
        let _guard = ENV_LOCK.lock().unwrap();
        let config = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", config.path()) };
        unsafe { std::env::set_var("XDG_STATE_HOME", state.path()) };
        let current = state.path().join("omarchy").join("current");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(current.join("theme.name"), "custom-theme\n").unwrap();

        assert_eq!(
            resolve_active(OMARCHY_SENTINEL).accent().fg,
            Some(ratatui::style::Color::Rgb(0x7a, 0xa2, 0xf7))
        );
    }
}
