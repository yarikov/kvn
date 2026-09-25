use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::model::Model;
use crate::config::profile::GeoRegion;
use crate::onboarding::{OnboardingStep, SetupState};
use crate::ui::layout::text::visual_width;

use super::overlay_footer;
use super::popup::centered_fixed_width_rect;

const ONBOARDING_POPUP_WIDTH: u16 = 56;
const TEXT_MARGIN: usize = 1;

pub(super) fn draw(frame: &mut Frame, model: &Model, step: OnboardingStep, area: Rect) {
    let horizontal_area = centered_fixed_width_rect(ONBOARDING_POPUP_WIDTH, area);
    let content_width = horizontal_area.width.saturating_sub(2).max(1) as usize;

    let build_lines = |compact: bool| {
        let mut lines = vec![Line::from(Span::styled(
            format!(
                "Getting started · {}/{}",
                step.index(model.onboarding.include_omarchy_card) + 1,
                OnboardingStep::total(model.onboarding.include_omarchy_card)
            ),
            model.theme.accent(),
        ))];
        let push_blank = |lines: &mut Vec<Line>| {
            if !compact {
                lines.push(Line::from(""));
            }
        };
        push_blank(&mut lines);
        lines.push(title_line(model, step));
        push_blank(&mut lines);
        for passage in body(model, step) {
            match passage {
                Passage::Blank => push_blank(&mut lines),
                Passage::Text(runs) => lines.extend(wrap(&runs, content_width)),
            }
        }
        push_blank(&mut lines);
        lines.push(Line::from(footer(model, step)));
        lines
    };

    let measure = |lines: &[Line]| -> u16 {
        lines
            .iter()
            .map(|line| line.width().max(1).div_ceil(content_width) as u16)
            .sum()
    };
    let mut lines = build_lines(false);
    let mut content_height = measure(&lines);
    if content_height.saturating_add(2) > area.height {
        lines = build_lines(true);
        content_height = measure(&lines);
    }

    let popup_height = content_height.saturating_add(2).min(area.height);
    let popup_area = Rect::new(
        horizontal_area.x,
        area.y + area.height.saturating_sub(popup_height) / 2,
        horizontal_area.width,
        popup_height,
    );
    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(model.theme.accent())
        .style(model.theme.popup_bg());
    let paragraph = Paragraph::new(lines)
        .style(model.theme.normal())
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph.block(block), popup_area);
}

fn title_line(model: &Model, step: OnboardingStep) -> Line<'static> {
    Line::from(Span::styled(title(model, step), model.theme.normal())).centered()
}

/// A stretch of a paragraph sharing one style.
struct Run {
    text: String,
    style: Option<Style>,
}

/// One body paragraph. Prose is stored unbroken so it can be filled to the
/// card's width at render time; the card is the only place that knows it.
enum Passage {
    Blank,
    Text(Vec<Run>),
}

fn text(value: impl Into<String>) -> Passage {
    Passage::Text(vec![Run {
        text: value.into(),
        style: None,
    }])
}

fn styled(value: impl Into<String>, style: Style) -> Passage {
    Passage::Text(vec![Run {
        text: value.into(),
        style: Some(style),
    }])
}

impl Run {
    fn plain(text: &str) -> Self {
        Self {
            text: text.to_string(),
            style: None,
        }
    }

    fn key(text: &str, style: Style) -> Self {
        Self {
            text: text.to_string(),
            style: Some(style),
        }
    }
}

/// A paragraph mixing prose with picked-out keys or commands.
fn runs(runs: Vec<Run>) -> Passage {
    Passage::Text(runs)
}

/// The common single-key case of [`runs`].
fn marked(before: &str, marked: &str, after: &str, style: Style) -> Passage {
    runs(vec![
        Run::plain(before),
        Run::key(marked, style),
        Run::plain(after),
    ])
}

/// Fill the runs across `width`, breaking between words only and indenting each
/// row by the card's margin. Styles travel with their words, so a highlighted
/// stretch keeps its colour wherever the break lands.
fn wrap(runs: &[Run], width: usize) -> Vec<Line<'static>> {
    let usable = width.saturating_sub(TEXT_MARGIN * 2).max(1);
    let indent = " ".repeat(TEXT_MARGIN);
    let mut lines = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut row_width = 0;
    for (word, style) in runs.iter().flat_map(|run| {
        run.text
            .split_whitespace()
            .map(move |word| (word.to_string(), run.style))
    }) {
        let word_width = visual_width(&word);
        let added = if spans.is_empty() {
            word_width
        } else {
            1 + word_width
        };
        if !spans.is_empty() && row_width + added > usable {
            lines.push(Line::from(std::mem::take(&mut spans)).left_aligned());
            row_width = 0;
        }
        if spans.is_empty() {
            spans.push(Span::raw(indent.clone()));
        } else {
            spans.push(Span::raw(" "));
        }
        spans.push(match style {
            Some(style) => Span::styled(word, style),
            None => Span::raw(word),
        });
        row_width += added;
    }
    if !spans.is_empty() {
        lines.push(Line::from(spans).left_aligned());
    }
    lines
}

fn title(model: &Model, step: OnboardingStep) -> &'static str {
    match step {
        OnboardingStep::Welcome => "Welcome to kvn",
        OnboardingStep::Region => "Choose a region",
        OnboardingStep::Routing => match country_region(model) {
            Some(_) => "Choose a routing mode",
            None => "Routing mode",
        },
        OnboardingStep::Profiles => "Add your first profile",
        OnboardingStep::Connected => "You’re connected!",
        OnboardingStep::AutoConnect => "Set up auto-connect",
        OnboardingStep::KillSwitch => "Set up the kill switch",
        OnboardingStep::Doctor => "Check your setup",
        OnboardingStep::Omarchy => "Set up Omarchy integration",
        OnboardingStep::Finish => "You’re all set",
    }
}

fn primary_action(model: &Model, step: OnboardingStep) -> &'static str {
    match step {
        OnboardingStep::Welcome => "start",
        OnboardingStep::Region => "choose a region",
        OnboardingStep::Routing => match country_region(model) {
            Some(_) => "choose a mode",
            None => "continue",
        },
        OnboardingStep::Profiles => "open profile list",
        OnboardingStep::Connected => "continue",
        OnboardingStep::AutoConnect => "continue",
        OnboardingStep::KillSwitch => "continue",
        OnboardingStep::Doctor => "continue",
        OnboardingStep::Omarchy => "continue",
        OnboardingStep::Finish => "finish",
    }
}

fn footer(model: &Model, step: OnboardingStep) -> String {
    let mut footer = overlay_footer(model, Some(primary_action(model, step)), false, false);
    if step == OnboardingStep::Profiles {
        footer.push_str(" · p add profile");
    }
    if command(model, step).is_some() {
        footer.push_str(" · y copy command");
    }
    footer
}

fn command(model: &Model, step: OnboardingStep) -> Option<&'static str> {
    step.command(model.integration_setup)
}

/// The command line a card shows, styled so it reads as something to run — and
/// sourced from the same place `y` copies, so the two cannot diverge.
fn command_line(model: &Model, step: OnboardingStep) -> Passage {
    styled(
        command(model, step).unwrap_or_default(),
        model.theme.success(),
    )
}

fn body(model: &Model, step: OnboardingStep) -> Vec<Passage> {
    match step {
        OnboardingStep::Welcome => vec![text(
            "This quick tour will help you get kvn set up and ready to use \
             by guiding you through routing, your first VPN profile, \
             auto-connect, and the kill switch.",
        )],
        OnboardingStep::Region => vec![text(
            "Your region helps kvn pick the right routing rules and makes \
             the relevant routing modes available. Choose Global if you \
             don’t need country-specific modes.",
        )],
        OnboardingStep::Routing => routing_body(model),
        OnboardingStep::Profiles => {
            let key = model.theme.success();
            vec![
                marked(
                    "Copy a VPN share link or subscription URL, then press ",
                    "p",
                    " to add it to kvn.",
                    key,
                ),
                Passage::Blank,
                marked(
                    "Open the profile list, select a profile, and press ",
                    "Enter",
                    " to connect.",
                    key,
                ),
            ]
        }
        OnboardingStep::Connected => vec![
            text("Your first VPN connection is up and running."),
            Passage::Blank,
            text("Next, let’s make sure everything is set up correctly."),
        ],
        OnboardingStep::AutoConnect => {
            protection_body(model, &AUTO_CONNECT_CARD, model.integration_setup.polkit)
        }
        OnboardingStep::KillSwitch => protection_body(
            model,
            &KILL_SWITCH_CARD,
            model.integration_setup.kill_switch,
        ),
        OnboardingStep::Doctor => vec![
            text("If something isn’t working, run:"),
            Passage::Blank,
            command_line(model, step),
            Passage::Blank,
            text(
                "It checks your kvn setup and points out problems with clear \
                 instructions on how to fix them.",
            ),
        ],
        OnboardingStep::Omarchy => match model.integration_setup.omarchy_plugin {
            true => vec![text(
                "kvn is integrated with your Omarchy desktop: the widget is on your bar.",
            )],
            false => vec![
                text("Run this command to integrate kvn with your Omarchy desktop:"),
                Passage::Blank,
                command_line(model, step),
            ],
        },
        OnboardingStep::Finish => {
            let key = model.theme.success();
            vec![
                text("kvn is ready to use."),
                Passage::Blank,
                runs(vec![
                    Run::plain("Press "),
                    Run::key("?", key),
                    Run::plain(" anytime for help and "),
                    Run::key("Space", key),
                    Run::plain(" to open settings."),
                ]),
            ]
        }
    }
}

/// The selected region when it actually has country modes to offer. `Global`
/// and an unset region both read as "no country-specific modes".
fn country_region(model: &Model) -> Option<GeoRegion> {
    match model.config.settings.geo_routing.current_region {
        Some(GeoRegion::Global) | None => None,
        Some(region) => Some(region),
    }
}

fn routing_body(model: &Model) -> Vec<Passage> {
    let modes = match country_region(model) {
        Some(region) => {
            let code = region.code_upper();
            format!(
                "Bypass {code} keeps {code} traffic outside the VPN, while \
                 Only {code} routes only {code} traffic through it."
            )
        }
        None => "Country-specific routing modes become available when you \
                 select a country region."
            .to_string(),
    };
    vec![
        text("Global routes all traffic through the VPN."),
        Passage::Blank,
        text(modes),
    ]
}

/// One of the two protection cards, as data: everything that differs between
/// auto-connect and the kill switch, so a single renderer covers the three
/// setup states.
struct ProtectionCard {
    step: OnboardingStep,
    intro: &'static str,
    setup_lead: &'static str,
    pending_lead: &'static str,
    toggle: &'static str,
    subject: &'static str,
}

const AUTO_CONNECT_CARD: ProtectionCard = ProtectionCard {
    step: OnboardingStep::AutoConnect,
    intro: "Auto-connect reconnects to your last used profile when the kvn daemon starts.",
    setup_lead: "First, set up polkit:",
    pending_lead: "Polkit setup is complete. Reboot once to activate it, then press ",
    toggle: "Shift+A",
    subject: "auto-connect",
};

const KILL_SWITCH_CARD: ProtectionCard = ProtectionCard {
    step: OnboardingStep::KillSwitch,
    intro: "The kill switch prevents new internet connections from going outside the VPN \
            if the tunnel goes down.",
    setup_lead: "First, set it up:",
    pending_lead: "Kill switch setup is complete. Reboot once to activate it, then press ",
    toggle: "Shift+K",
    subject: "the kill switch",
};

/// Both integrations gate on the same `kvn-tui` group, so a single reboot
/// activates them and the copy asks for one.
fn protection_body(model: &Model, card: &ProtectionCard, setup: SetupState) -> Vec<Passage> {
    let key = model.theme.success();
    let toggle_sentence = |lead: &str| {
        runs(vec![
            Run::plain(lead),
            Run::key(card.toggle, key),
            Run::plain(&format!(" to toggle {}.", card.subject)),
        ])
    };
    let mut passages = vec![text(card.intro), Passage::Blank];
    match setup {
        SetupState::Missing => passages.extend([
            text(card.setup_lead),
            Passage::Blank,
            command_line(model, card.step),
            Passage::Blank,
            toggle_sentence(match model.integration_setup.group_active {
                true => "Then press ",
                false => "Reboot once, then press ",
            }),
        ]),
        SetupState::PendingReboot => passages.push(toggle_sentence(card.pending_lead)),
        SetupState::Ready => passages.push(toggle_sentence("Press ")),
    }
    passages
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::Overlay;
    use crate::test_helpers::{
        APP_WINDOW_COLS, APP_WINDOW_ROWS, model_with_profiles, snapshot_styles, snapshot_terminal,
    };
    use crate::ui::layout::{MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH};

    fn card(step: OnboardingStep) -> Model {
        let mut model = model_with_profiles(vec![]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.onboarding.state.step = step;
        // Render every card, Omarchy included, with one consistent counter.
        model.onboarding.include_omarchy_card = true;
        model.overlay = Overlay::Onboarding(step);
        model
    }

    #[test]
    fn draw_every_onboarding_card_snapshot() {
        for step in OnboardingStep::ORDER {
            let model = card(step);
            let label = format!("{step:?}").to_lowercase();
            insta::with_settings!({snapshot_suffix => label}, {
                insta::assert_snapshot!(snapshot_terminal(
                    &model,
                    APP_WINDOW_COLS,
                    APP_WINDOW_ROWS
                ));
            });
        }
    }

    #[test]
    fn without_omarchy_the_last_card_closes_a_shorter_tour() {
        let mut model = card(OnboardingStep::Finish);
        model.onboarding.include_omarchy_card = false;
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    #[test]
    fn draw_routing_card_for_the_global_region_snapshot() {
        let mut model = card(OnboardingStep::Routing);
        model
            .config
            .settings
            .geo_routing
            .set_region(GeoRegion::Global);
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    #[test]
    fn draw_onboarding_cards_at_the_minimum_terminal_size_snapshot() {
        for step in [OnboardingStep::Welcome, OnboardingStep::KillSwitch] {
            let model = card(step);
            let label = format!("{step:?}").to_lowercase();
            insta::with_settings!({snapshot_suffix => label}, {
                insta::assert_snapshot!(snapshot_terminal(
                    &model,
                    MIN_TERMINAL_WIDTH,
                    MIN_TERMINAL_HEIGHT
                ));
            });
        }
    }

    #[test]
    fn protection_cards_follow_their_setup_state_snapshot() {
        // `Missing` with an inactive group is the default the every-card sweep
        // already renders; here it pairs with an active group, which asks for no
        // reboot.
        let cases = [
            (SetupState::Missing, true, "missing-groupactive"),
            (SetupState::PendingReboot, false, "pendingreboot"),
            (SetupState::Ready, true, "ready"),
        ];
        for step in [OnboardingStep::AutoConnect, OnboardingStep::KillSwitch] {
            for (state, group_active, name) in cases {
                let mut model = card(step);
                model.integration_setup = crate::onboarding::IntegrationSetup {
                    polkit: state,
                    kill_switch: state,
                    group_active,
                    omarchy_plugin: false,
                };
                let label = format!("{step:?}-{name}").to_lowercase();
                insta::with_settings!({snapshot_suffix => label}, {
                    insta::assert_snapshot!(snapshot_terminal(
                        &model,
                        APP_WINDOW_COLS,
                        APP_WINDOW_ROWS
                    ));
                });
            }
        }
    }

    #[test]
    fn the_omarchy_card_confirms_an_installed_plugin_snapshot() {
        let mut model = card(OnboardingStep::Omarchy);
        model.integration_setup.omarchy_plugin = true;
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    #[test]
    fn the_keys_and_commands_a_card_asks_for_are_picked_out() {
        for step in [
            OnboardingStep::Profiles,
            OnboardingStep::Doctor,
            OnboardingStep::Omarchy,
            OnboardingStep::AutoConnect,
            OnboardingStep::KillSwitch,
        ] {
            let model = card(step);
            let label = format!("{step:?}").to_lowercase();
            insta::with_settings!({snapshot_suffix => label}, {
                insta::assert_snapshot!(snapshot_styles(
                    &model,
                    APP_WINDOW_COLS,
                    APP_WINDOW_ROWS
                ));
            });
        }
    }
}
