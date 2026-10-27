use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use crate::app::model::{Model, RoutingPageLine};
use crate::config::routing::{ModeRule, Outbound};
use crate::ui::layout::scrollbar::ScrollWindow;
use crate::ui::layout::text::{fit_to_visual_width, visual_width};

use super::popup::{
    row_window_start, selection_visible, settings_content_width, settings_row_width,
};
use super::settings_row::{settings_full_width_line, settings_value_layout, settings_value_line};

const RULES_HEADING: &str = "Rules (top wins)";
const MOVED_RULES_HEADING: &str = "Rules (top wins) *";
const FINAL_KEY: &str = "final";
const MAX_RULE_VALUE_WIDTH: usize = 34;
const COLUMN_GAP: &str = "  ";

pub(super) struct RoutingPage {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) popup_width: u16,
    pub(super) scroll: ScrollWindow,
}

struct RuleRow {
    key: &'static str,
    value: String,
    route: &'static str,
}

struct RuleColumns {
    key: usize,
    value: usize,
    route: usize,
}

impl RuleColumns {
    fn of(rows: &[RuleRow]) -> Self {
        let widest = |width: fn(&RuleRow) -> usize| rows.iter().map(width).max().unwrap_or(0);
        Self {
            key: widest(|row| visual_width(row.key)),
            value: widest(|row| visual_width(&row.value)).min(MAX_RULE_VALUE_WIDTH),
            route: widest(|row| visual_width(row.route)),
        }
    }

    fn width(&self) -> usize {
        self.key + self.value + self.route + COLUMN_GAP.len() * 2
    }

    fn text(&self, row: &RuleRow) -> String {
        [
            fit_to_visual_width(row.key, self.key),
            fit_to_visual_width(&row.value, self.value),
            fit_to_visual_width(row.route, self.route),
        ]
        .join(COLUMN_GAP)
    }
}

pub(super) fn routing_page(model: &Model, area: Rect, frame_widths: &[usize]) -> RoutingPage {
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
    let final_outbound = model
        .routing
        .mode(shown)
        .map_or(Outbound::Proxy, |mode| mode.final_outbound);
    let rule_rows: Vec<RuleRow> = model
        .shown_mode_rules()
        .iter()
        .map(rule_row)
        .chain(std::iter::once(RuleRow {
            key: FINAL_KEY,
            value: String::new(),
            route: final_outbound.tag(),
        }))
        .collect();
    let columns = RuleColumns::of(&rule_rows);
    let rules_heading = if model.shown_rule_order_draft().is_some() {
        MOVED_RULES_HEADING
    } else {
        RULES_HEADING
    };
    let content_widths: Vec<usize> = frame_widths
        .iter()
        .copied()
        .chain([columns.width(), visual_width(MOVED_RULES_HEADING)])
        .collect();
    let layout = settings_value_layout(&settings, stable_value_width, area, &content_widths);
    let row_width = settings_row_width(layout.popup_width);
    let content_width = settings_content_width(layout.popup_width);
    let indent = " ".repeat(content_width.saturating_sub(columns.width()) / 2);

    let rows = crate::app::scroll::routing_settings_rows(model);
    let selected = model.routing_settings_selected();
    let visible = selection_visible(area, rows.len());
    let start = row_window_start(&rows, visible, selected, model.overlay_scroll);
    let theme = &model.theme;
    let lines = model
        .routing_page_lines()
        .into_iter()
        .skip(start)
        .take(visible)
        .map(|line| match line {
            RoutingPageLine::Row(0) => {
                let (name, value, dirty) = &settings[0];
                settings_value_line(name, value, *dirty, selected == 0, layout, theme)
            }
            RoutingPageLine::Row(row) => settings_full_width_line(
                &format!("{indent}{}", columns.text(&rule_rows[row - 1])),
                row_width,
                row == selected,
                theme,
            ),
            RoutingPageLine::Blank => Line::from(""),
            RoutingPageLine::RulesHeading => {
                Line::from(Span::styled(rules_heading, theme.accent())).centered()
            }
        })
        .collect();
    RoutingPage {
        lines,
        popup_width: layout.popup_width,
        scroll: ScrollWindow {
            total: rows.len(),
            visible,
            start,
        },
    }
}

fn rule_row(rule: &ModeRule) -> RuleRow {
    RuleRow {
        key: rule.matches.key(),
        value: rule.matches.value().to_string(),
        route: rule.route.tag(),
    }
}
