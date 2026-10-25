use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::model::{Model, SettingsMenuEntry};
use crate::config::profile::IconSet;
use crate::ui::icons::icons;
use crate::ui::layout::text::visual_width;

use super::popup::{
    settings_content_sized_rect, settings_content_width, settings_popup_width, settings_row_width,
};
use super::settings_row::{settings_full_width_line, settings_value_layout, settings_value_line};
use super::{APPLY_ACTION, SETTINGS_HEADING, overlay_footer, settings_heading};

pub(super) fn draw(
    frame: &mut Frame,
    model: &Model,
    page: crate::app::model::SettingsMenuPage,
    area: Rect,
) {
    use crate::app::model::SettingsMenuPage;
    let heading = page
        .entry()
        .map_or_else(|| SETTINGS_HEADING.to_string(), settings_heading);
    let footer = overlay_footer(
        model,
        (page != SettingsMenuPage::Root).then_some(APPLY_ACTION),
        true,
        page != SettingsMenuPage::Root,
    );
    let frame_widths = [visual_width(&heading), visual_width(&footer)];
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(heading, model.theme.accent())).centered(),
        Line::from(""),
    ];
    let popup_width = match page {
        SettingsMenuPage::Root => {
            let entries =
                SettingsMenuEntry::ALL.map(|entry| (entry.key().to_string(), entry.label()));
            let entry_width = entries
                .iter()
                .map(|(key, action)| visual_width(&format!("{key}  {action}")))
                .max()
                .unwrap_or(0);
            let popup_width =
                settings_popup_width(area, frame_widths.into_iter().chain([entry_width]));
            let row_width = settings_row_width(popup_width);
            let content_width = settings_content_width(popup_width);
            let entry_indent = " ".repeat(content_width.saturating_sub(entry_width) / 2);
            for (index, (key, action)) in entries.into_iter().enumerate() {
                let command = format!("{key}  {action}");
                let row = format!("{entry_indent}{command}");
                let line = if index == model.settings_menu_selected {
                    settings_full_width_line(&row, row_width, true, &model.theme)
                } else {
                    Line::from(vec![
                        Span::raw(format!(" {}", entry_indent)),
                        Span::styled(key, model.theme.accent().add_modifier(Modifier::BOLD)),
                        Span::raw(" "),
                        Span::styled(icons(model.icon_set()).arrow, model.theme.accent()),
                        Span::raw(" "),
                        Span::styled(action, model.theme.normal()),
                    ])
                };
                lines.push(line);
            }
            popup_width
        }
        SettingsMenuPage::Routing => {
            let shown = model.shown_routing_mode();
            let settings = vec![(
                "Mode",
                model.routing.mode_name(shown),
                shown != model.config.settings.geo_routing.mode(&model.routing),
            )];
            let stable_value_width = model
                .routing
                .mode_ids()
                .into_iter()
                .map(|id| visual_width(&model.routing.mode_name(id)))
                .max()
                .unwrap_or(0);
            let layout = settings_value_layout(&settings, stable_value_width, area, &frame_widths);
            for (index, (name, value, dirty)) in settings.into_iter().enumerate() {
                lines.push(settings_value_line(
                    name,
                    &value,
                    dirty,
                    index == model.settings_menu_selected,
                    layout,
                    &model.theme,
                ));
            }
            layout.popup_width
        }
        SettingsMenuPage::Connection => {
            let saved_auto_connect = model.config.settings.auto_connect;
            let saved_kill_switch = model.config.settings.kill_switch;
            let shown = model.shown_connection_settings();
            let settings = [
                (
                    "Auto-connect",
                    if shown.auto_connect { "on" } else { "off" }.to_string(),
                    shown.auto_connect != saved_auto_connect,
                ),
                (
                    "Kill switch",
                    if shown.kill_switch { "on" } else { "off" }.to_string(),
                    shown.kill_switch != saved_kill_switch,
                ),
            ];
            let stable_value_width = ["on", "off"]
                .into_iter()
                .map(visual_width)
                .max()
                .unwrap_or(0);
            let layout = settings_value_layout(&settings, stable_value_width, area, &frame_widths);
            for (index, (name, value, dirty)) in settings.into_iter().enumerate() {
                lines.push(settings_value_line(
                    name,
                    &value,
                    dirty,
                    index == model.settings_menu_selected,
                    layout,
                    &model.theme,
                ));
            }
            layout.popup_width
        }
        SettingsMenuPage::Interface => {
            let shown_theme = model
                .theme_draft
                .as_deref()
                .unwrap_or(&model.config.settings.theme);
            let shown_icons = model.icon_set();
            let settings = [
                (
                    "Theme",
                    shorten_theme_name(theme_value_label(shown_theme)),
                    shown_theme != model.config.settings.theme,
                ),
                (
                    "Icons",
                    shown_icons.label().to_string(),
                    shown_icons != model.config.settings.icons,
                ),
            ];
            let stable_value_width = crate::ui::palette::Palette::bundled_names()
                .into_iter()
                .map(shorten_theme_name)
                .chain(
                    [
                        AUTO_THEME_LABEL,
                        IconSet::Nerd.label(),
                        IconSet::Unicode.label(),
                    ]
                    .map(str::to_string),
                )
                .map(|value| visual_width(&value))
                .max()
                .unwrap_or(0);
            let layout = settings_value_layout(&settings, stable_value_width, area, &frame_widths);
            for (index, (name, value, dirty)) in settings.into_iter().enumerate() {
                lines.push(settings_value_line(
                    name,
                    &value,
                    dirty,
                    index == model.settings_menu_selected,
                    layout,
                    &model.theme,
                ));
            }
            layout.popup_width
        }
    };
    lines.push(Line::from(""));
    lines.push(Line::from(footer).centered());

    let popup_area = settings_content_sized_rect(area, popup_width, &lines);
    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(model.theme.accent())
        .style(model.theme.popup_bg());
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);
    frame.render_widget(Paragraph::new(lines).style(model.theme.normal()), inner);
}

pub(super) const AUTO_THEME_LABEL: &str = "auto";

const MAX_THEME_NAME_CHARS: usize = 12;

fn shorten_theme_name(name: &str) -> String {
    if name.chars().count() <= MAX_THEME_NAME_CHARS {
        return name.to_string();
    }
    let kept: String = name.chars().take(MAX_THEME_NAME_CHARS - 1).collect();
    format!("{kept}…")
}

pub(super) fn theme_value_label(slug: &str) -> &str {
    if slug == crate::config::profile::OMARCHY_THEME_SENTINEL {
        AUTO_THEME_LABEL
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {

    use crate::app::model::Overlay;
    use crate::test_helpers::{
        APP_WINDOW_COLS, APP_WINDOW_ROWS, model_with_profiles, snapshot_terminal,
    };

    #[test]
    fn draw_settings_menu_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::SettingsMenu(crate::app::model::SettingsMenuPage::Root);
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    #[test]
    fn draw_interface_menu_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.theme = "tokyo-night".into();
        model.overlay = Overlay::SettingsMenu(crate::app::model::SettingsMenuPage::Interface);
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    #[test]
    fn draw_interface_menu_drafts_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::SettingsMenu(crate::app::model::SettingsMenuPage::Interface);
        model.theme_draft = Some("catppuccin-latte".into());
        model.interface_settings_draft = Some(crate::config::profile::IconSet::Unicode);
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    #[test]
    fn draw_routing_menu_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::SettingsMenu(crate::app::model::SettingsMenuPage::Routing);
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    #[test]
    fn draw_routing_menu_country_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model
            .routing
            .add_region_modes(crate::config::profile::GeoRegion::Ru);
        model.config.settings.geo_routing.set_mode("bypass_ru");
        model.overlay = Overlay::SettingsMenu(crate::app::model::SettingsMenuPage::Routing);
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    #[test]
    fn draw_connection_menu_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::SettingsMenu(crate::app::model::SettingsMenuPage::Connection);
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }
}
