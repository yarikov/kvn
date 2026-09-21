use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::GeoAutoUpdate;

/// Selected geo region for rule-set downloads and routing mode availability.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum GeoRegion {
    Global,
    Ru,
    Cn,
    Ir,
}

impl GeoRegion {
    /// All regions in their canonical UI / cycle order. Single source of truth
    /// for region listings — adding a country here propagates to overlays,
    /// key navigation, and runner availability scans.
    pub const ALL: [GeoRegion; 4] = [
        GeoRegion::Ru,
        GeoRegion::Cn,
        GeoRegion::Ir,
        GeoRegion::Global,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            GeoRegion::Global => "global",
            GeoRegion::Ru => "ru",
            GeoRegion::Cn => "cn",
            GeoRegion::Ir => "ir",
        }
    }

    /// Uppercase two-letter code used in user-facing labels ("Bypass RU").
    pub fn code_upper(&self) -> &'static str {
        match self {
            GeoRegion::Global => "GLOBAL",
            GeoRegion::Ru => "RU",
            GeoRegion::Cn => "CN",
            GeoRegion::Ir => "IR",
        }
    }
}

/// Routing mode for geoip/geosite rules. Generic over the active geo region so
/// adding a country requires no new variants — `RoutingMode::Bypass(GeoRegion::Br)`
/// just works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RoutingMode {
    #[default]
    Global,
    Bypass(GeoRegion),
    Only(GeoRegion),
}

impl RoutingMode {
    /// Return the list of routing modes available for the given geo region.
    pub fn available(region: Option<GeoRegion>) -> Vec<RoutingMode> {
        match region {
            Some(r) if !matches!(r, GeoRegion::Global) => vec![
                RoutingMode::Global,
                RoutingMode::Bypass(r),
                RoutingMode::Only(r),
            ],
            _ => vec![RoutingMode::Global],
        }
    }
}

impl std::fmt::Display for RoutingMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoutingMode::Global => f.write_str("Global"),
            RoutingMode::Bypass(r) => write!(f, "Bypass {}", r.code_upper()),
            RoutingMode::Only(r) => write!(f, "Only {}", r.code_upper()),
        }
    }
}

// Custom (de)serialization preserves the legacy on-disk shape
// ("global", "bypass_ru", "only_cn", ...) so existing profiles.json files load.
impl Serialize for RoutingMode {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            RoutingMode::Global => s.serialize_str("global"),
            RoutingMode::Bypass(r) => s.serialize_str(&format!("bypass_{}", r.as_str())),
            RoutingMode::Only(r) => s.serialize_str(&format!("only_{}", r.as_str())),
        }
    }
}

impl<'de> Deserialize<'de> for RoutingMode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let s = String::deserialize(d)?;
        if s == "global" {
            return Ok(RoutingMode::Global);
        }
        let parse_region = |code: &str| -> Option<GeoRegion> {
            GeoRegion::ALL.iter().copied().find(|r| r.as_str() == code)
        };
        if let Some(code) = s.strip_prefix("bypass_") {
            return parse_region(code).map(RoutingMode::Bypass).ok_or_else(|| {
                D::Error::custom(format!("unknown region in routing mode `{}`", s))
            });
        }
        if let Some(code) = s.strip_prefix("only_") {
            return parse_region(code).map(RoutingMode::Only).ok_or_else(|| {
                D::Error::custom(format!("unknown region in routing mode `{}`", s))
            });
        }
        Err(D::Error::custom(format!("unknown routing mode `{}`", s)))
    }
}

/// Routing override for one well-known service, applied ahead of the
/// regional geo rules so it wins in every routing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ServiceRoute {
    /// No override — the service follows the regional routing mode.
    #[default]
    Disabled,
    /// Force the service through the VPN tunnel, even under `Bypass`.
    Proxy,
    /// Send the service out the `direct` outbound (real network location),
    /// even under `Only`. Deliberately bypasses the tunnel — and the kill
    /// switch, which allowlists the `direct` outbound's fwmark by design.
    Direct,
}

impl ServiceRoute {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Disabled => "Disabled",
            Self::Proxy => "Proxy",
            Self::Direct => "Direct",
        }
    }

    /// Cycle order in the TUI overlay: Disabled → Proxy → Direct → Disabled.
    pub const fn next(self) -> Self {
        match self {
            Self::Disabled => Self::Proxy,
            Self::Proxy => Self::Direct,
            Self::Direct => Self::Disabled,
        }
    }

    pub const fn prev(self) -> Self {
        match self {
            Self::Disabled => Self::Direct,
            Self::Proxy => Self::Disabled,
            Self::Direct => Self::Proxy,
        }
    }
}

/// Services with predefined rule-sets that can be routed individually (see
/// [`ServiceRoute`]). Adding a service = a variant here, an `ALL` entry, and
/// a descriptor arm in `geo::service_assets`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutedService {
    Steam,
    Telegram,
}

impl RoutedService {
    /// Display / rule-generation order. Iterate this instead of the
    /// `service_routes` map — `HashMap` iteration order is nondeterministic,
    /// which would make the generated sing-box config unstable across runs.
    pub const ALL: [Self; 2] = [Self::Steam, Self::Telegram];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Steam => "Steam",
            Self::Telegram => "Telegram",
        }
    }
}

/// Geo-region and routing-mode preferences.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct GeoRouting {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_region: Option<GeoRegion>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub selected_region_modes: HashMap<GeoRegion, RoutingMode>,
    #[serde(default)]
    pub auto_update: GeoAutoUpdate,
    /// Per-service routing overrides. An absent entry means
    /// [`ServiceRoute::Disabled`] — overrides are strictly opt-in.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub service_routes: HashMap<RoutedService, ServiceRoute>,
}

impl GeoRouting {
    /// Return the active routing mode for the current region.
    /// Falls back to `Global` when no region is selected or no mode is stored.
    pub fn mode(&self) -> RoutingMode {
        self.current_region
            .and_then(|r| self.selected_region_modes.get(&r).copied())
            .unwrap_or(RoutingMode::Global)
    }

    /// Change the active geo region.
    pub fn set_region(&mut self, region: GeoRegion) {
        self.current_region = Some(region);
    }

    /// Store the routing mode for the current region.
    pub fn set_mode(&mut self, mode: RoutingMode) {
        if let Some(region) = self.current_region {
            self.selected_region_modes.insert(region, mode);
        }
    }

    /// Return the routing override for `service` (absent = `Disabled`).
    pub fn service_route(&self, service: RoutedService) -> ServiceRoute {
        self.service_routes
            .get(&service)
            .copied()
            .unwrap_or_default()
    }

    /// Services with an active (non-`Disabled`) routing override, in
    /// [`RoutedService::ALL`] order.
    pub fn enabled_services(&self) -> Vec<RoutedService> {
        RoutedService::ALL
            .into_iter()
            .filter(|s| self.service_route(*s) != ServiceRoute::Disabled)
            .collect()
    }

    /// Return routing modes available for the current region.
    pub fn available_modes(&self) -> Vec<RoutingMode> {
        RoutingMode::available(self.current_region)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::profile::*;

    #[test]
    fn routing_mode_display() {
        assert_eq!(RoutingMode::Global.to_string(), "Global");
        assert_eq!(RoutingMode::Bypass(GeoRegion::Ru).to_string(), "Bypass RU");
        assert_eq!(RoutingMode::Only(GeoRegion::Ru).to_string(), "Only RU");
        assert_eq!(RoutingMode::Bypass(GeoRegion::Cn).to_string(), "Bypass CN");
        assert_eq!(RoutingMode::Only(GeoRegion::Cn).to_string(), "Only CN");
        assert_eq!(RoutingMode::Bypass(GeoRegion::Ir).to_string(), "Bypass IR");
        assert_eq!(RoutingMode::Only(GeoRegion::Ir).to_string(), "Only IR");
    }

    #[test]
    fn geo_region_serializes_to_global() {
        let json = serde_json::to_string(&GeoRegion::Global).unwrap();
        assert_eq!(json, r#""global""#);
    }

    #[test]
    fn routing_mode_serde_round_trip_matches_legacy_wire_format() {
        // Every variant maps to its on-disk string and back, preserving
        // backward compatibility with profiles.json files written by builds
        // that used the flat enum (`bypass_ru`, `only_cn`, etc.).
        let cases: &[(RoutingMode, &str)] = &[
            (RoutingMode::Global, r#""global""#),
            (RoutingMode::Bypass(GeoRegion::Ru), r#""bypass_ru""#),
            (RoutingMode::Only(GeoRegion::Ru), r#""only_ru""#),
            (RoutingMode::Bypass(GeoRegion::Cn), r#""bypass_cn""#),
            (RoutingMode::Only(GeoRegion::Cn), r#""only_cn""#),
            (RoutingMode::Bypass(GeoRegion::Ir), r#""bypass_ir""#),
            (RoutingMode::Only(GeoRegion::Ir), r#""only_ir""#),
        ];
        for (mode, wire) in cases {
            assert_eq!(
                serde_json::to_string(mode).unwrap(),
                *wire,
                "ser {:?}",
                mode
            );
            let parsed: RoutingMode = serde_json::from_str(wire).unwrap();
            assert_eq!(parsed, *mode, "de {}", wire);
        }
    }

    #[test]
    fn routing_mode_deserialize_rejects_unknown_strings() {
        assert!(serde_json::from_str::<RoutingMode>(r#""bogus""#).is_err());
        assert!(serde_json::from_str::<RoutingMode>(r#""bypass_xx""#).is_err());
        assert!(serde_json::from_str::<RoutingMode>(r#""only_""#).is_err());
    }

    #[test]
    fn geo_region_all_contains_every_variant() {
        // Smoke test: GeoRegion::ALL must enumerate all enum variants.
        // The match-statement below fails to compile if a variant is added
        // to the enum without being listed in ALL.
        for r in GeoRegion::ALL {
            match r {
                GeoRegion::Global | GeoRegion::Ru | GeoRegion::Cn | GeoRegion::Ir => {}
            }
        }
        assert_eq!(GeoRegion::ALL.len(), 4);
    }

    #[test]
    fn routing_mode_available() {
        assert_eq!(RoutingMode::available(None), vec![RoutingMode::Global]);
        for region in [GeoRegion::Ru, GeoRegion::Cn, GeoRegion::Ir] {
            assert_eq!(
                RoutingMode::available(Some(region)),
                vec![
                    RoutingMode::Global,
                    RoutingMode::Bypass(region),
                    RoutingMode::Only(region),
                ],
                "region {:?}",
                region,
            );
        }
        assert_eq!(
            RoutingMode::available(Some(GeoRegion::Global)),
            vec![RoutingMode::Global]
        );
    }

    #[test]
    fn geo_routing_mode_falls_back_to_global() {
        let g = GeoRouting::default();
        assert_eq!(g.mode(), RoutingMode::Global);
    }

    #[test]
    fn geo_routing_set_mode_persists_per_region() {
        let mut g = GeoRouting::default();
        g.set_region(GeoRegion::Ru);
        g.set_mode(RoutingMode::Bypass(GeoRegion::Ru));
        assert_eq!(g.mode(), RoutingMode::Bypass(GeoRegion::Ru));
        assert_eq!(
            g.selected_region_modes.get(&GeoRegion::Ru),
            Some(&RoutingMode::Bypass(GeoRegion::Ru))
        );

        g.set_region(GeoRegion::Cn);
        g.set_mode(RoutingMode::Only(GeoRegion::Cn));
        assert_eq!(g.mode(), RoutingMode::Only(GeoRegion::Cn));
        g.set_region(GeoRegion::Ru);
        assert_eq!(g.mode(), RoutingMode::Bypass(GeoRegion::Ru));
    }

    #[test]
    fn geo_routing_available_modes_uses_current_region() {
        let mut g = GeoRouting::default();
        assert_eq!(g.available_modes(), vec![RoutingMode::Global]);
        g.set_region(GeoRegion::Ru);
        assert_eq!(
            g.available_modes(),
            vec![
                RoutingMode::Global,
                RoutingMode::Bypass(GeoRegion::Ru),
                RoutingMode::Only(GeoRegion::Ru)
            ]
        );
    }

    #[test]
    fn service_routes_default_to_disabled() {
        // Opt-in: overriding a service's route must never happen without an
        // explicit user decision — absent map entries mean Disabled.
        let g = GeoRouting::default();
        assert!(g.service_routes.is_empty());
        for service in RoutedService::ALL {
            assert_eq!(g.service_route(service), ServiceRoute::Disabled);
        }
        assert!(g.enabled_services().is_empty());
    }

    #[test]
    fn service_routes_absent_in_json_deserialize_as_empty() {
        let json = r#"{
            "tun_interface": "kvn0",
            "dns_strategy": "prefer_ipv4",
            "geo_routing": {},
            "auto_connect": false
        }"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert!(s.geo_routing.service_routes.is_empty());
        // Empty map is skipped on serialize — no noise in profiles.json.
        assert!(
            !serde_json::to_string(&s)
                .unwrap()
                .contains("service_routes")
        );
    }

    #[test]
    fn service_routes_round_trip_with_snake_case_keys() {
        let mut s = Settings::default();
        s.geo_routing
            .service_routes
            .insert(RoutedService::Steam, ServiceRoute::Direct);
        s.geo_routing
            .service_routes
            .insert(RoutedService::Telegram, ServiceRoute::Proxy);
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"steam\":\"direct\""), "{json}");
        assert!(json.contains("\"telegram\":\"proxy\""), "{json}");
        let restored: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored.geo_routing.service_route(RoutedService::Steam),
            ServiceRoute::Direct
        );
        assert_eq!(
            restored.geo_routing.service_route(RoutedService::Telegram),
            ServiceRoute::Proxy
        );
    }

    #[test]
    fn enabled_services_follow_all_order() {
        let mut g = GeoRouting::default();
        // Inserted in reverse of ALL order; a Disabled entry is excluded.
        g.service_routes
            .insert(RoutedService::Telegram, ServiceRoute::Proxy);
        g.service_routes
            .insert(RoutedService::Steam, ServiceRoute::Disabled);
        assert_eq!(g.enabled_services(), vec![RoutedService::Telegram]);
        g.service_routes
            .insert(RoutedService::Steam, ServiceRoute::Direct);
        assert_eq!(
            g.enabled_services(),
            vec![RoutedService::Steam, RoutedService::Telegram]
        );
    }

    #[test]
    fn service_route_cycle_covers_all_states() {
        let mut r = ServiceRoute::Disabled;
        let mut seen = Vec::new();
        for _ in 0..3 {
            r = r.next();
            seen.push(r);
        }
        assert_eq!(
            seen,
            vec![
                ServiceRoute::Proxy,
                ServiceRoute::Direct,
                ServiceRoute::Disabled
            ]
        );
        for route in seen {
            assert_eq!(route.next().prev(), route);
        }
    }
}
