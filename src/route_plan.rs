use crate::config::profile::{GeoRegion, RoutingFallback, RoutingMode, Settings};
use crate::config::routing::RoutingConfig;
use crate::geo::RuleSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outbound {
    Proxy,
    Direct,
}

impl Outbound {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Proxy => "proxy",
            Self::Direct => "direct",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEntry {
    pub rule_set: RuleSet,
    pub outbound: Outbound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutePlan {
    pub entries: Vec<PlanEntry>,
    pub private_direct: bool,
    pub final_outbound: Outbound,
    pub fallback: Option<RoutingFallback>,
}

impl RoutePlan {
    pub fn for_mode(mode: RoutingMode, routing: &RoutingConfig) -> Self {
        match mode {
            RoutingMode::Global => Self {
                entries: Vec::new(),
                private_direct: false,
                final_outbound: Outbound::Proxy,
                fallback: None,
            },
            RoutingMode::Bypass(region) => {
                Self::region(mode, region, routing, Outbound::Direct, Outbound::Proxy)
            }
            RoutingMode::Only(region) => {
                Self::region(mode, region, routing, Outbound::Proxy, Outbound::Direct)
            }
        }
    }

    pub fn for_settings(settings: &Settings, routing: &RoutingConfig) -> Self {
        Self::for_mode(settings.geo_routing.mode(), routing)
    }

    fn region(
        mode: RoutingMode,
        region: GeoRegion,
        routing: &RoutingConfig,
        region_outbound: Outbound,
        final_outbound: Outbound,
    ) -> Self {
        Self {
            entries: crate::geo::region_rule_set_tags(region)
                .iter()
                .filter_map(|tag| routing.rule_set(tag))
                .map(|rule_set| PlanEntry {
                    rule_set,
                    outbound: region_outbound,
                })
                .collect(),
            private_direct: region != GeoRegion::Global,
            final_outbound,
            fallback: Some(RoutingFallback::for_mode(mode)),
        }
    }

    pub fn rule_sets(&self) -> impl Iterator<Item = &RuleSet> + '_ {
        self.entries.iter().map(|entry| &entry.rule_set)
    }
}

pub fn required_rule_sets(settings: &Settings, routing: &RoutingConfig) -> Vec<RuleSet> {
    RoutePlan::for_settings(settings, routing)
        .entries
        .into_iter()
        .map(|entry| entry.rule_set)
        .collect()
}

pub fn selectable_rule_sets(settings: &Settings, routing: &RoutingConfig) -> Vec<RuleSet> {
    let mut selectable: Vec<RuleSet> = Vec::new();
    for mode in settings.geo_routing.available_modes() {
        for entry in RoutePlan::for_mode(mode, routing).entries {
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

    fn tags_and_outbounds(plan: &RoutePlan) -> Vec<(&str, Outbound)> {
        plan.entries
            .iter()
            .map(|entry| (&*entry.rule_set.tag, entry.outbound))
            .collect()
    }

    #[test]
    fn global_routes_everything_through_the_proxy_without_rule_sets() {
        let plan = RoutePlan::for_mode(RoutingMode::Global, &RoutingConfig::default());
        assert!(plan.entries.is_empty());
        assert!(!plan.private_direct);
        assert_eq!(plan.final_outbound, Outbound::Proxy);
        assert_eq!(plan.fallback, None);
    }

    #[test]
    fn bypass_sends_the_region_direct_and_falls_back_to_global() {
        let plan = RoutePlan::for_mode(
            RoutingMode::Bypass(GeoRegion::Ru),
            &RoutingConfig::default(),
        );
        assert_eq!(
            tags_and_outbounds(&plan),
            [
                ("geosite-category-ru", Outbound::Direct),
                ("geoip-ru", Outbound::Direct),
            ]
        );
        assert!(plan.private_direct);
        assert_eq!(plan.final_outbound, Outbound::Proxy);
        assert_eq!(plan.fallback, Some(RoutingFallback::Global));
    }

    #[test]
    fn only_tunnels_the_region_and_falls_back_to_direct() {
        let plan = RoutePlan::for_mode(RoutingMode::Only(GeoRegion::Cn), &RoutingConfig::default());
        assert_eq!(
            tags_and_outbounds(&plan),
            [
                ("geosite-cn", Outbound::Proxy),
                ("geoip-cn", Outbound::Proxy)
            ]
        );
        assert!(plan.private_direct);
        assert_eq!(plan.final_outbound, Outbound::Direct);
        assert_eq!(plan.fallback, Some(RoutingFallback::Direct));
    }

    #[test]
    fn a_region_routes_the_rule_sets_routing_json_declares_for_its_tags() {
        let mut routing = RoutingConfig::default();
        routing.rule_sets.get_mut("geoip-ru").unwrap().url =
            "https://mirror.example/geoip-ru.srs".into();
        routing.rule_sets.remove("geosite-category-ru");

        let plan = RoutePlan::for_mode(RoutingMode::Bypass(GeoRegion::Ru), &routing);

        let urls: Vec<&str> = plan.rule_sets().map(|rule_set| &*rule_set.url).collect();
        assert_eq!(urls, ["https://mirror.example/geoip-ru.srs"]);
    }

    #[test]
    fn a_region_mode_for_the_global_region_routes_no_rule_sets() {
        let plan = RoutePlan::for_mode(
            RoutingMode::Bypass(GeoRegion::Global),
            &RoutingConfig::default(),
        );
        assert!(plan.entries.is_empty());
        assert!(!plan.private_direct);
    }

    #[test]
    fn required_rule_sets_are_those_of_the_active_mode() {
        let mut settings = Settings::default();
        assert!(required_rule_sets(&settings, &RoutingConfig::default()).is_empty());

        settings.geo_routing.set_region(GeoRegion::Ir);
        settings
            .geo_routing
            .set_mode(RoutingMode::Only(GeoRegion::Ir));
        let tags: Vec<String> = required_rule_sets(&settings, &RoutingConfig::default())
            .into_iter()
            .map(|rule_set| rule_set.tag.into_owned())
            .collect();
        assert_eq!(tags, ["geosite-category-ir", "geoip-ir"]);
    }

    #[test]
    fn selectable_rule_sets_are_those_of_every_mode_of_the_region_once() {
        let mut settings = Settings::default();
        assert!(selectable_rule_sets(&settings, &RoutingConfig::default()).is_empty());

        settings.geo_routing.set_region(GeoRegion::Ru);
        assert_eq!(settings.geo_routing.mode(), RoutingMode::Global);
        let tags: Vec<String> = selectable_rule_sets(&settings, &RoutingConfig::default())
            .into_iter()
            .map(|rule_set| rule_set.tag.into_owned())
            .collect();
        assert_eq!(tags, ["geosite-category-ru", "geoip-ru"]);
    }
}
