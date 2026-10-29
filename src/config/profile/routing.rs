use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::GeoAutoUpdate;
use crate::config::routing::{GLOBAL_MODE_ID, RoutingConfig};

/// Country whose default routing modes the first-run tour adds to `routing.json`.
#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, JsonSchema, clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum GeoRegion {
    /// Only the Global mode, which sends everything through the tunnel.
    Global,
    /// Russia: the Bypass RU and Only RU modes.
    Ru,
    /// China: the Bypass CN and Only CN modes.
    Cn,
    /// Iran: the Bypass IR and Only IR modes.
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

    pub fn of_default_mode(id: &str) -> Option<GeoRegion> {
        if id == GLOBAL_MODE_ID {
            return Some(GeoRegion::Global);
        }
        let code = id
            .strip_prefix("bypass_")
            .or_else(|| id.strip_prefix("only_"))?;
        GeoRegion::ALL
            .into_iter()
            .find(|region| *region != GeoRegion::Global && region.as_str() == code)
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

fn global_mode_id() -> String {
    GLOBAL_MODE_ID.to_string()
}

/// Routing-mode selection and rule-set update preferences.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct GeoRouting {
    /// Id of the selected mode in `routing.json`, such as `bypass_ru`. An id
    /// the file does not declare routes as `global`.
    #[serde(default = "global_mode_id")]
    pub mode: String,
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
    #[serde(default, rename = "current_region", skip_serializing)]
    #[schemars(skip)]
    pub(crate) legacy_current_region: Option<GeoRegion>,
    #[serde(default, rename = "selected_region_modes", skip_serializing)]
    #[schemars(skip)]
    pub(crate) legacy_selected_region_modes: HashMap<GeoRegion, String>,
}

impl Default for GeoRouting {
    fn default() -> Self {
        Self {
            mode: global_mode_id(),
            auto_update: GeoAutoUpdate::default(),
            legacy_service_routes: None,
            legacy_current_region: None,
            legacy_selected_region_modes: HashMap::new(),
        }
    }
}

impl GeoRouting {
    pub fn mode<'a>(&'a self, routing: &RoutingConfig) -> &'a str {
        if routing.declares(&self.mode) {
            &self.mode
        } else {
            GLOBAL_MODE_ID
        }
    }

    pub fn set_mode(&mut self, id: &str) {
        self.mode = id.to_string();
    }

    pub(crate) fn select_legacy_region_mode(&mut self) {
        self.mode = self
            .legacy_current_region
            .and_then(|region| self.legacy_selected_region_modes.get(&region))
            .cloned()
            .unwrap_or_else(global_mode_id);
    }

    pub(crate) fn predates_mode_selection(&self) -> bool {
        self.legacy_current_region.is_some()
    }

    pub(crate) fn forget_legacy_regions(&mut self) {
        self.legacy_current_region = None;
        self.legacy_selected_region_modes.clear();
    }

    pub fn used_regions(&self) -> Vec<GeoRegion> {
        GeoRegion::ALL
            .into_iter()
            .filter(|region| {
                *region != GeoRegion::Global
                    && (GeoRegion::of_default_mode(&self.mode) == Some(*region)
                        || self.legacy_current_region == Some(*region)
                        || self.legacy_selected_region_modes.contains_key(region))
            })
            .collect()
    }

    // TODO: drop once the omakvn plugin selects modes by id (TASK-26); it
    // still reads current_region and selected_region_modes from the snapshot.
    pub(crate) fn plugin_region_fields(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut fields = serde_json::Map::new();
        let Some(region) = GeoRegion::of_default_mode(&self.mode) else {
            return fields;
        };
        fields.insert("current_region".into(), region.as_str().into());
        let mut selected = serde_json::Map::new();
        if region != GeoRegion::Global {
            selected.insert(region.as_str().into(), self.mode.clone().into());
        }
        fields.insert("selected_region_modes".into(), selected.into());
        fields
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::profile::*;

    #[test]
    fn a_mode_missing_from_routing_json_reads_as_global() {
        let mut routing = GeoRouting::default();
        routing.set_mode("only_ru");
        let mut config = RoutingConfig::default();
        assert_eq!(routing.mode(&config), "only_ru");

        config.modes.retain(|mode| mode.id != "only_ru");
        assert_eq!(routing.mode(&config), "global");
    }

    #[test]
    fn default_selection_is_global_and_is_written() {
        let routing = GeoRouting::default();
        assert_eq!(routing.mode(&RoutingConfig::default()), "global");
        assert_eq!(
            serde_json::to_value(&routing).unwrap()["mode"],
            serde_json::json!("global")
        );
        let parsed: GeoRouting = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed, routing);
    }

    #[test]
    fn default_mode_ids_name_their_region() {
        let cases = [
            ("global", Some(GeoRegion::Global)),
            ("bypass_ru", Some(GeoRegion::Ru)),
            ("only_cn", Some(GeoRegion::Cn)),
            ("bypass_ir", Some(GeoRegion::Ir)),
            ("bypass_global", None),
            ("only_xx", None),
            ("work", None),
        ];
        for (id, region) in cases {
            assert_eq!(GeoRegion::of_default_mode(id), region, "{id}");
        }
    }

    #[test]
    fn geo_region_serializes_to_global() {
        let json = serde_json::to_string(&GeoRegion::Global).unwrap();
        assert_eq!(json, r#""global""#);
    }

    #[test]
    fn legacy_regions_are_read_but_never_written() {
        let document = serde_json::json!({
            "current_region": "ru",
            "selected_region_modes": { "ru": "only_ru", "cn": "bypass_cn" }
        });
        let mut routing: GeoRouting = serde_json::from_value(document).unwrap();
        assert!(routing.predates_mode_selection());
        assert_eq!(routing.used_regions(), [GeoRegion::Ru, GeoRegion::Cn]);

        routing.select_legacy_region_mode();
        assert_eq!(routing.mode, "only_ru");
        let written = serde_json::to_value(&routing).unwrap();
        assert!(written.get("current_region").is_none());
        assert!(written.get("selected_region_modes").is_none());

        routing.forget_legacy_regions();
        assert!(!routing.predates_mode_selection());
        assert_eq!(routing.used_regions(), [GeoRegion::Ru]);
    }

    #[test]
    fn a_legacy_region_without_a_stored_mode_selects_global() {
        let mut routing: GeoRouting =
            serde_json::from_value(serde_json::json!({ "current_region": "cn" })).unwrap();
        routing.set_mode("bypass_ru");
        routing.select_legacy_region_mode();
        assert_eq!(routing.mode, "global");
        assert_eq!(routing.used_regions(), [GeoRegion::Cn]);
    }

    #[test]
    fn plugin_region_fields_follow_the_selected_mode() {
        let fields = |mode: &str| {
            let mut routing = GeoRouting::default();
            routing.set_mode(mode);
            serde_json::Value::Object(routing.plugin_region_fields())
        };
        assert_eq!(
            fields("only_ru"),
            serde_json::json!({
                "current_region": "ru",
                "selected_region_modes": { "ru": "only_ru" }
            })
        );
        assert_eq!(
            fields("global"),
            serde_json::json!({ "current_region": "global", "selected_region_modes": {} })
        );
        assert_eq!(fields("work"), serde_json::json!({}));
    }

    #[test]
    fn service_routes_of_an_older_build_survive_a_save_unchanged() {
        let service_routes = serde_json::json!({ "steam": "direct", "telegram": "proxy" });
        let document = serde_json::json!({
            "geo_routing": { "mode": "bypass_ru", "service_routes": service_routes }
        });
        let settings: Settings = serde_json::from_value(document).unwrap();
        assert_eq!(settings.geo_routing.mode, "bypass_ru");
        assert_eq!(
            serde_json::to_value(&settings).unwrap()["geo_routing"]["service_routes"],
            service_routes
        );
    }
}
