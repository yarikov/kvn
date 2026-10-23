use crate::config::profile::{GeoRegion, RoutingFallback, RoutingMode, Settings};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanEntry {
    pub rule_set: &'static RuleSet,
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
    pub fn for_mode(mode: RoutingMode) -> Self {
        match mode {
            RoutingMode::Global => Self {
                entries: Vec::new(),
                private_direct: false,
                final_outbound: Outbound::Proxy,
                fallback: None,
            },
            RoutingMode::Bypass(region) => {
                Self::region(mode, region, Outbound::Direct, Outbound::Proxy)
            }
            RoutingMode::Only(region) => {
                Self::region(mode, region, Outbound::Proxy, Outbound::Direct)
            }
        }
    }

    pub fn for_settings(settings: &Settings) -> Self {
        Self::for_mode(settings.geo_routing.mode())
    }

    fn region(
        mode: RoutingMode,
        region: GeoRegion,
        region_outbound: Outbound,
        final_outbound: Outbound,
    ) -> Self {
        Self {
            entries: crate::geo::region_rule_sets(region)
                .iter()
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

    pub fn rule_sets(&self) -> impl Iterator<Item = &'static RuleSet> + '_ {
        self.entries.iter().map(|entry| entry.rule_set)
    }
}

pub fn required_rule_sets(settings: &Settings) -> Vec<&'static RuleSet> {
    RoutePlan::for_settings(settings).rule_sets().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags_and_outbounds(plan: &RoutePlan) -> Vec<(&'static str, Outbound)> {
        plan.entries
            .iter()
            .map(|entry| (entry.rule_set.tag, entry.outbound))
            .collect()
    }

    #[test]
    fn global_routes_everything_through_the_proxy_without_rule_sets() {
        let plan = RoutePlan::for_mode(RoutingMode::Global);
        assert!(plan.entries.is_empty());
        assert!(!plan.private_direct);
        assert_eq!(plan.final_outbound, Outbound::Proxy);
        assert_eq!(plan.fallback, None);
    }

    #[test]
    fn bypass_sends_the_region_direct_and_falls_back_to_global() {
        let plan = RoutePlan::for_mode(RoutingMode::Bypass(GeoRegion::Ru));
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
        let plan = RoutePlan::for_mode(RoutingMode::Only(GeoRegion::Cn));
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
    fn a_region_mode_for_the_global_region_routes_no_rule_sets() {
        let plan = RoutePlan::for_mode(RoutingMode::Bypass(GeoRegion::Global));
        assert!(plan.entries.is_empty());
        assert!(!plan.private_direct);
    }

    #[test]
    fn required_rule_sets_are_those_of_the_active_mode() {
        let mut settings = Settings::default();
        assert!(required_rule_sets(&settings).is_empty());

        settings.geo_routing.set_region(GeoRegion::Ir);
        settings
            .geo_routing
            .set_mode(RoutingMode::Only(GeoRegion::Ir));
        let tags: Vec<&str> = required_rule_sets(&settings)
            .iter()
            .map(|rule_set| rule_set.tag)
            .collect();
        assert_eq!(tags, ["geosite-category-ir", "geoip-ir"]);
    }
}
