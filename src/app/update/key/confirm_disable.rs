use crate::app::effect::Effect;
use crate::app::model::{DisableTarget, Model, Overlay};
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::update::key::settings_menu::{set_auto_connect, set_kill_switch};

pub(in crate::app::update) fn toggle_auto_connect(model: &mut Model) -> Vec<Effect> {
    if model.config.settings.auto_connect && !model.auto_connect_pending {
        model.overlay = Overlay::ConfirmDisable(DisableTarget::AutoConnect);
        return vec![];
    }
    set_auto_connect(model, true)
}

pub(in crate::app::update) fn toggle_kill_switch(model: &mut Model) -> Vec<Effect> {
    if model.config.settings.kill_switch && model.kill_switch_pending.is_none() {
        model.overlay = Overlay::ConfirmDisable(DisableTarget::KillSwitch);
        return vec![];
    }
    set_kill_switch(model, true)
}

pub(in crate::app::update) fn handle_confirm_disable(
    model: &mut Model,
    target: DisableTarget,
    key: KeyEvent,
) -> Vec<Effect> {
    match key.code {
        KeyCode::Char('y') | KeyCode::Enter => {
            model.overlay = Overlay::None;
            match target {
                DisableTarget::AutoConnect => set_auto_connect(model, false),
                DisableTarget::KillSwitch => set_kill_switch(model, false),
            }
        }
        KeyCode::Char('n') | KeyCode::Char('q') | KeyCode::Esc => {
            model.overlay = Overlay::None;
            vec![]
        }
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::update::key::handle_key;
    use crate::app::update::key::sources::handle_sources;
    use crate::test_helpers::*;

    fn model_with_auto_connect(enabled: bool) -> Model {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.auto_connect = enabled;
        model
    }

    fn model_with_kill_switch(enabled: bool) -> Model {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.kill_switch = enabled;
        model
    }

    #[test]
    fn shift_a_asks_before_disabling_auto_connect() {
        let mut model = model_with_auto_connect(true);
        let effects = handle_sources(&mut model, key('A'));
        assert!(effects.is_empty());
        assert_eq!(
            model.overlay,
            Overlay::ConfirmDisable(DisableTarget::AutoConnect)
        );
        assert!(model.config.settings.auto_connect);
    }

    #[test]
    fn deprecated_a_behaves_like_shift_a() {
        let mut model = model_with_auto_connect(true);
        let effects = handle_sources(&mut model, key('a'));
        assert!(effects.is_empty());
        assert_eq!(
            model.overlay,
            Overlay::ConfirmDisable(DisableTarget::AutoConnect)
        );
    }

    #[test]
    fn confirming_disables_auto_connect() {
        for confirm in [key('y'), enter()] {
            let mut model = model_with_auto_connect(true);
            handle_sources(&mut model, key('A'));
            let effects = handle_key(&mut model, confirm);
            assert_eq!(model.overlay, Overlay::None);
            assert!(!model.config.settings.auto_connect);
            assert!(effects.contains(&Effect::SaveConfig));
        }
    }

    #[test]
    fn cancelling_keeps_auto_connect_enabled() {
        for cancel in [key('n'), key('q'), esc()] {
            let mut model = model_with_auto_connect(true);
            handle_sources(&mut model, key('A'));
            let effects = handle_key(&mut model, cancel);
            assert!(effects.is_empty());
            assert_eq!(model.overlay, Overlay::None);
            assert!(model.config.settings.auto_connect);
        }
    }

    #[test]
    fn unknown_key_keeps_the_dialog_open() {
        let mut model = model_with_auto_connect(true);
        handle_sources(&mut model, key('A'));
        let effects = handle_key(&mut model, key('x'));
        assert!(effects.is_empty());
        assert_eq!(
            model.overlay,
            Overlay::ConfirmDisable(DisableTarget::AutoConnect)
        );
    }

    #[test]
    fn shift_k_asks_before_disabling_the_kill_switch() {
        let mut model = model_with_kill_switch(true);
        let effects = handle_sources(&mut model, key('K'));
        assert!(effects.is_empty());
        assert_eq!(
            model.overlay,
            Overlay::ConfirmDisable(DisableTarget::KillSwitch)
        );
        assert_eq!(model.kill_switch_pending, None);

        let effects = handle_key(&mut model, key('y'));
        assert_eq!(model.overlay, Overlay::None);
        assert_eq!(model.kill_switch_pending, Some(false));
        assert!(effects.contains(&Effect::ApplyKillSwitch { enabled: false }));
    }

    #[test]
    fn an_in_flight_apply_skips_the_kill_switch_dialog() {
        let mut model = model_with_kill_switch(true);
        model.kill_switch_pending = Some(false);
        let effects = handle_sources(&mut model, key('K'));
        assert!(effects.is_empty());
        assert_eq!(model.overlay, Overlay::None);
    }

    #[test]
    fn an_in_flight_polkit_check_skips_the_auto_connect_dialog() {
        let mut model = model_with_auto_connect(true);
        model.auto_connect_pending = true;
        let effects = handle_sources(&mut model, key('A'));
        assert!(effects.is_empty());
        assert_eq!(model.overlay, Overlay::None);
        assert!(model.config.settings.auto_connect);
    }
}
