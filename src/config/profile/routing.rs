use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::GeoAutoUpdate;
use crate::config::routing::RoutingConfig;

/// Selected geo region for rule-set downloads and routing mode availability.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum GeoRegion {
    Global,
    Ru,
    Cn,
    Ir,
}

impl GeoRegion {
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

    pub fn code_upper(&self) -> &'static str {
        match self {
            GeoRegion::Global => "GLOBAL",
            GeoRegion::Ru => "RU",
            GeoRegion::Cn => "CN",
            GeoRegion::Ir => "IR",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RoutingMode {
    #[default]
    Global,
    Bypass(GeoRegion),
    Only(GeoRegion),
}

impl RoutingMode {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingFallback {
    Global,
    Direct,
}

impl RoutingFallback {
    pub fn to(outbound: crate::config::routing::Outbound) -> Self {
        match outbound {
            crate::config::routing::Outbound::Proxy => Self::Global,
            crate::config::routing::Outbound::Direct => Self::Direct,
        }
    }
}

impl std::fmt::Display for RoutingFallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Global => f.write_str("Global"),
            Self::Direct => f.write_str("Direct"),
        }
    }
}

impl RoutingMode {
    pub fn id(self) -> String {
        match self {
            RoutingMode::Global => "global".to_string(),
            RoutingMode::Bypass(r) => format!("bypass_{}", r.as_str()),
            RoutingMode::Only(r) => format!("only_{}", r.as_str()),
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

impl Serialize for RoutingMode {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.id())
    }
}

impl JsonSchema for RoutingMode {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "RoutingMode".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let mut modes = Vec::new();
        for mode in GeoRegion::ALL
            .iter()
            .flat_map(|region| RoutingMode::available(Some(*region)))
        {
            let value = serde_json::to_value(mode).expect("RoutingMode serializes to a string");
            if !modes.contains(&value) {
                modes.push(value);
            }
        }
        schemars::json_schema!({ "type": "string", "enum": modes })
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

/// Geo-region and routing-mode preferences.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, JsonSchema)]
pub struct GeoRouting {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_region: Option<GeoRegion>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub selected_region_modes: HashMap<GeoRegion, RoutingMode>,
    #[serde(default)]
    pub auto_update: GeoAutoUpdate,
    /// Steam and Telegram overrides written by v0.35 and earlier, kept
    /// verbatim until they migrate to custom rule-sets and routing rules.
    #[serde(
        default,
        rename = "service_routes",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(skip)]
    pub(crate) legacy_service_routes: Option<serde_json::Value>,
}

impl GeoRouting {
    pub fn mode(&self, routing: &RoutingConfig) -> RoutingMode {
        self.current_region
            .and_then(|r| self.selected_region_modes.get(&r).copied())
            .filter(|mode| {
                RoutingMode::available(self.current_region).contains(mode)
                    && routing.declares(*mode)
            })
            .unwrap_or(RoutingMode::Global)
    }

    pub fn set_region(&mut self, region: GeoRegion) {
        self.current_region = Some(region);
    }

    pub fn set_mode(&mut self, mode: RoutingMode) {
        if let Some(region) = self.current_region {
            self.selected_region_modes.insert(region, mode);
        }
    }

    pub fn used_regions(&self) -> Vec<GeoRegion> {
        GeoRegion::ALL
            .into_iter()
            .filter(|region| {
                *region != GeoRegion::Global
                    && (self.current_region == Some(*region)
                        || self.selected_region_modes.contains_key(region))
            })
            .collect()
    }

    pub fn available_modes(&self, routing: &RoutingConfig) -> Vec<RoutingMode> {
        routing.modes_of(self.current_region)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::profile::*;

    #[test]
    fn a_mode_stored_for_another_region_reads_as_global() {
        let mut routing = GeoRouting::default();
        routing.set_region(GeoRegion::Ru);
        routing.set_mode(RoutingMode::Bypass(GeoRegion::Cn));
        assert_eq!(routing.mode(&RoutingConfig::default()), RoutingMode::Global);

        routing.set_region(GeoRegion::Global);
        routing.set_mode(RoutingMode::Only(GeoRegion::Ru));
        assert_eq!(routing.mode(&RoutingConfig::default()), RoutingMode::Global);
    }

    #[test]
    fn a_mode_missing_from_routing_json_is_unavailable_and_reads_as_global() {
        let mut routing = GeoRouting::default();
        routing.set_region(GeoRegion::Ru);
        routing.set_mode(RoutingMode::Only(GeoRegion::Ru));
        let mut config = RoutingConfig::default();
        config.modes.retain(|mode| mode.id != "only_ru");

        assert_eq!(
            routing.available_modes(&config),
            [RoutingMode::Global, RoutingMode::Bypass(GeoRegion::Ru)]
        );
        assert_eq!(routing.mode(&config), RoutingMode::Global);
    }

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
        assert_eq!(g.mode(&RoutingConfig::default()), RoutingMode::Global);
    }

    #[test]
    fn geo_routing_set_mode_persists_per_region() {
        let mut g = GeoRouting::default();
        g.set_region(GeoRegion::Ru);
        g.set_mode(RoutingMode::Bypass(GeoRegion::Ru));
        assert_eq!(
            g.mode(&RoutingConfig::default()),
            RoutingMode::Bypass(GeoRegion::Ru)
        );
        assert_eq!(
            g.selected_region_modes.get(&GeoRegion::Ru),
            Some(&RoutingMode::Bypass(GeoRegion::Ru))
        );

        g.set_region(GeoRegion::Cn);
        g.set_mode(RoutingMode::Only(GeoRegion::Cn));
        assert_eq!(
            g.mode(&RoutingConfig::default()),
            RoutingMode::Only(GeoRegion::Cn)
        );
        g.set_region(GeoRegion::Ru);
        assert_eq!(
            g.mode(&RoutingConfig::default()),
            RoutingMode::Bypass(GeoRegion::Ru)
        );
    }

    #[test]
    fn geo_routing_available_modes_uses_current_region() {
        let mut g = GeoRouting::default();
        assert_eq!(
            g.available_modes(&RoutingConfig::default()),
            vec![RoutingMode::Global]
        );
        g.set_region(GeoRegion::Ru);
        assert_eq!(
            g.available_modes(&RoutingConfig::default()),
            vec![
                RoutingMode::Global,
                RoutingMode::Bypass(GeoRegion::Ru),
                RoutingMode::Only(GeoRegion::Ru)
            ]
        );
    }

    #[test]
    fn service_routes_of_an_older_build_survive_a_save_unchanged() {
        let service_routes = serde_json::json!({ "steam": "direct", "telegram": "proxy" });
        let document = serde_json::json!({
            "geo_routing": { "current_region": "ru", "service_routes": service_routes }
        });
        let settings: Settings = serde_json::from_value(document).unwrap();
        assert_eq!(settings.geo_routing.current_region, Some(GeoRegion::Ru));
        assert_eq!(
            serde_json::to_value(&settings).unwrap()["geo_routing"]["service_routes"],
            service_routes
        );
    }
}
