use crate::config::profile::{RoutingFallback, Settings};
pub use crate::config::routing::Outbound;
use crate::config::routing::{ModeDefinition, RoutingConfig};
use crate::geo::RuleSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEntry {
    pub rule_set: RuleSet,
    pub outbound: Outbound,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutePlan {
    pub entries: Vec<PlanEntry>,
    pub private_direct: bool,
    pub final_outbound: Outbound,
    pub fallback: RoutingFallback,
}

impl RoutePlan {
    pub fn for_mode(id: &str, routing: &RoutingConfig) -> Self {
        routing
            .mode(id)
            .map_or_else(Self::everything_through_proxy, |definition| {
                Self::for_definition(definition, routing)
            })
    }

    pub fn for_settings(settings: &Settings, routing: &RoutingConfig) -> Self {
        Self::for_mode(settings.geo_routing.mode(routing), routing)
    }

    fn for_definition(definition: &ModeDefinition, routing: &RoutingConfig) -> Self {
        Self {
            entries: definition
                .rules
                .iter()
                .filter_map(|rule| {
                    Some(PlanEntry {
                        rule_set: routing.rule_set(&rule.rule_set)?,
                        outbound: rule.route,
                        required: rule.required,
                    })
                })
                .collect(),
            private_direct: definition.private_direct,
            final_outbound: definition.final_outbound,
            fallback: RoutingFallback::to(definition.final_outbound),
        }
    }

    fn everything_through_proxy() -> Self {
        Self {
            entries: Vec::new(),
            private_direct: false,
            final_outbound: Outbound::Proxy,
            fallback: RoutingFallback::Global,
        }
    }

    pub fn rule_sets(&self) -> impl Iterator<Item = &RuleSet> + '_ {
        self.entries.iter().map(|entry| &entry.rule_set)
    }

    pub fn required_rule_sets(&self) -> impl Iterator<Item = &RuleSet> + '_ {
        self.entries
            .iter()
            .filter(|entry| entry.required)
            .map(|entry| &entry.rule_set)
    }
}

pub fn active_rule_sets(settings: &Settings, routing: &RoutingConfig) -> Vec<RuleSet> {
    RoutePlan::for_settings(settings, routing)
        .entries
        .into_iter()
        .map(|entry| entry.rule_set)
        .collect()
}

pub fn selectable_rule_sets(routing: &RoutingConfig) -> Vec<RuleSet> {
    let mut selectable: Vec<RuleSet> = Vec::new();
    for id in routing.mode_ids() {
        for entry in RoutePlan::for_mode(id, routing).entries {
            if !selectable.contains(&entry.rule_set) {
                selectable.push(entry.rule_set);
            }
        }
    }
    selectable
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::profile::{GeoRegion, GeoRouting};

    fn tags_and_outbounds(plan: &RoutePlan) -> Vec<(&str, Outbound)> {
        plan.entries
            .iter()
            .map(|entry| (&*entry.rule_set.tag, entry.outbound))
            .collect()
    }

    fn tags(rule_sets: Vec<RuleSet>) -> Vec<String> {
        rule_sets
            .into_iter()
            .map(|rule_set| rule_set.tag.into_owned())
            .collect()
    }

    #[test]
    fn global_routes_everything_through_the_proxy_without_rule_sets() {
        let plan = RoutePlan::for_mode("global", &RoutingConfig::default());
        assert!(plan.entries.is_empty());
        assert!(!plan.private_direct);
        assert_eq!(plan.final_outbound, Outbound::Proxy);
        assert_eq!(plan.fallback, RoutingFallback::Global);
    }

    #[test]
    fn bypass_sends_the_region_direct_and_falls_back_to_global() {
        let plan = RoutePlan::for_mode("bypass_ru", &RoutingConfig::default());
        assert_eq!(
            tags_and_outbounds(&plan),
            [
                ("geosite-category-ru", Outbound::Direct),
                ("geoip-ru", Outbound::Direct),
            ]
        );
        assert!(plan.private_direct);
        assert_eq!(plan.final_outbound, Outbound::Proxy);
        assert_eq!(plan.fallback, RoutingFallback::Global);
    }

    #[test]
    fn only_tunnels_the_region_and_falls_back_to_direct() {
        let plan = RoutePlan::for_mode("only_cn", &RoutingConfig::default());
        assert_eq!(
            tags_and_outbounds(&plan),
            [
                ("geosite-cn", Outbound::Proxy),
                ("geoip-cn", Outbound::Proxy)
            ]
        );
        assert!(plan.private_direct);
        assert_eq!(plan.final_outbound, Outbound::Direct);
        assert_eq!(plan.fallback, RoutingFallback::Direct);
    }

    #[test]
    fn a_region_routes_the_rule_sets_routing_json_declares_for_its_tags() {
        let mut routing = RoutingConfig::default();
        routing.rule_sets.get_mut("geoip-ru").unwrap().url =
            "https://mirror.example/geoip-ru.srs".into();
        routing.rule_sets.remove("geosite-category-ru");

        let plan = RoutePlan::for_mode("bypass_ru", &routing);

        let urls: Vec<&str> = plan.rule_sets().map(|rule_set| &*rule_set.url).collect();
        assert_eq!(urls, ["https://mirror.example/geoip-ru.srs"]);
    }

    #[test]
    fn a_mode_missing_from_routing_json_routes_no_rule_sets() {
        let plan = RoutePlan::for_mode("bypass_global", &RoutingConfig::default());
        assert!(plan.entries.is_empty());
        assert!(!plan.private_direct);
    }

    #[test]
    fn active_rule_sets_are_those_of_the_active_mode() {
        let mut settings = Settings::default();
        assert!(active_rule_sets(&settings, &RoutingConfig::default()).is_empty());

        settings.geo_routing.set_mode("only_ir");
        assert_eq!(
            tags(active_rule_sets(&settings, &RoutingConfig::default())),
            ["geosite-category-ir", "geoip-ir"]
        );
    }

    #[test]
    fn selectable_rule_sets_are_those_of_every_mode_in_routing_json_once() {
        let mut routing = RoutingConfig::for_new_file(&GeoRouting::default());
        assert!(selectable_rule_sets(&routing).is_empty());

        routing.add_region_modes(GeoRegion::Ru);
        assert_eq!(
            tags(selectable_rule_sets(&routing)),
            ["geosite-category-ru", "geoip-ru"]
        );
    }
}
