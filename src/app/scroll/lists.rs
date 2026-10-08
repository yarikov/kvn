use crate::app::model::{Model, Overlay, RoutingSettingsItem, SettingsMenuPage, SourceRow};
use crate::config::profile::{GeoRegion, RoutedService};

pub(crate) fn context(overlay: Overlay) -> Overlay {
    match overlay {
        Overlay::Help(mut state) => {
            state.selected = 0;
            Overlay::Help(state)
        }
        other => other,
    }
}

pub(crate) fn source_rows(model: &Model) -> Vec<Option<usize>> {
    let rows = model.source_rows();
    let mut visual = Vec::new();
    let mut previous_group = None;
    for (index, row) in rows.iter().enumerate() {
        let group = match row {
            SourceRow::StandaloneProfile(_) => None,
            SourceRow::SubscriptionHeader(index) => Some(*index),
            SourceRow::SubscriptionProfile { sub_idx, .. } => Some(*sub_idx),
        };
        if !visual.is_empty() && group != previous_group {
            visual.push(None);
        }
        visual.push(Some(index));
        previous_group = group;
    }
    visual
}

pub(crate) fn list(model: &Model) -> Option<(Vec<Option<usize>>, usize)> {
    let (len, selected) = match model.overlay {
        Overlay::None => return Some((source_rows(model), model.selected)),
        Overlay::Help(state) => {
            let rows = crate::ui::help::rows(state.context)
                .iter()
                .enumerate()
                .map(|(index, row)| {
                    matches!(row, crate::ui::help::HelpLine::Command { .. }).then_some(index)
                })
                .collect();
            return Some((rows, state.selected));
        }
        Overlay::ThemeSettings => (
            crate::app::update::theme_picker_slugs().len(),
            model.theme_selected,
        ),
        Overlay::RoutingMode => (
            model.config.settings.geo_routing.available_modes().len(),
            model.routing_selected,
        ),
        Overlay::GeoRegions => (GeoRegion::ALL.len(), model.geo_region_selected),
        Overlay::DnsSettings => (3, model.dns_selected),
        Overlay::ServiceRouting => (RoutedService::ALL.len(), model.service_routing_selected),
        Overlay::SettingsMenu(page) => {
            let len = match page {
                SettingsMenuPage::Root => 4,
                SettingsMenuPage::Routing => {
                    RoutingSettingsItem::available(model.shown_routing_settings().region).len()
                }
                _ => 2,
            };
            (len, model.settings_menu_selected)
        }
        Overlay::Support => (3, model.support_selected),
        _ => return None,
    };
    Some(((0..len).map(Some).collect(), selected))
}

pub(crate) fn select(model: &mut Model, selected: usize) {
    match &mut model.overlay {
        Overlay::None => model.selected = selected,
        Overlay::Help(state) => state.selected = selected,
        Overlay::ThemeSettings => {
            model.theme_selected = selected;
            model.theme_draft = crate::app::update::theme_picker_slugs()
                .get(selected)
                .cloned();
        }
        Overlay::RoutingMode => model.routing_selected = selected,
        Overlay::GeoRegions => model.geo_region_selected = selected,
        Overlay::DnsSettings => model.dns_selected = selected,
        Overlay::ServiceRouting => model.service_routing_selected = selected,
        Overlay::SettingsMenu(_) => model.settings_menu_selected = selected,
        Overlay::Support => model.support_selected = selected,
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::{HelpState, RoutingSettingsDraft};

    #[test]
    fn each_selectable_context_updates_its_own_selection() {
        let _lock = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let mut model = crate::test_helpers::model_with_subscription();
        model.routing_settings_draft = Some(RoutingSettingsDraft {
            region: Some(GeoRegion::Ru),
            ..Default::default()
        });
        for overlay in [
            Overlay::None,
            Overlay::GeoRegions,
            Overlay::RoutingMode,
            Overlay::DnsSettings,
            Overlay::ServiceRouting,
            Overlay::ThemeSettings,
            Overlay::SettingsMenu(SettingsMenuPage::Root),
            Overlay::SettingsMenu(SettingsMenuPage::Routing),
            Overlay::SettingsMenu(SettingsMenuPage::Connection),
            Overlay::SettingsMenu(SettingsMenuPage::Interface),
            Overlay::Support,
            Overlay::Help(HelpState::default()),
        ] {
            model.overlay = overlay;
            let (rows, _) = list(&model).unwrap();
            let last = *rows.iter().flatten().last().unwrap();
            select(&mut model, last);
            assert_eq!(list(&model).unwrap().1, last);
        }
        model.overlay = Overlay::ThemeSettings;
        select(&mut model, 1);
        assert_eq!(
            model.theme_draft,
            crate::app::update::theme_picker_slugs().get(1).cloned()
        );
        model.overlay = Overlay::ConfirmDelete;
        assert!(list(&model).is_none());
        select(&mut model, 1);
        assert_eq!(model.overlay, Overlay::ConfirmDelete);
    }

    #[test]
    fn help_context_does_not_depend_on_selection() {
        let mut state = HelpState::default();
        let original = context(Overlay::Help(state));
        state.selected += 5;
        assert_eq!(context(Overlay::Help(state)), original);
    }
}
