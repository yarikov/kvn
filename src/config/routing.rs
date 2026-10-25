use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::json_pointer::escape_token;
use super::profile::{
    ConfigDiagnostic, GeoRegion, GeoRouting, into_result, schema_diagnostics_against,
};
use crate::geo::RuleSet;

pub const ROUTING_SCHEMA_VERSION: u32 = 1;

static SCHEMA: LazyLock<Value> = LazyLock::new(routing_json_schema);

const LEGACY_SERVICE_RULE_SETS: &[(&str, [RuleSet; 2])] = &[
    (
        "steam",
        [
            RuleSet::builtin(
                "geosite-steam",
                RuleSetKind::Geosite,
                "https://raw.githubusercontent.com/MetaCubeX/meta-rules-dat/sing/geo/geosite/steam.srs",
            ),
            RuleSet::builtin(
                "geoip-steam",
                RuleSetKind::Geoip,
                "https://raw.githubusercontent.com/MetaCubeX/meta-rules-dat/sing/asn/AS32590.srs",
            ),
        ],
    ),
    (
        "telegram",
        [
            RuleSet::builtin(
                "geosite-telegram",
                RuleSetKind::Geosite,
                "https://raw.githubusercontent.com/MetaCubeX/meta-rules-dat/sing/geo/geosite/telegram.srs",
            ),
            RuleSet::builtin(
                "geoip-telegram",
                RuleSetKind::Geoip,
                "https://raw.githubusercontent.com/MetaCubeX/meta-rules-dat/sing/geo/geoip/telegram.srs",
            ),
        ],
    ),
];

/// What a rule-set matches. kvn does not read the file, so the kind is
/// whatever the rule-set is declared as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RuleSetKind {
    /// IP networks, matched once the destination is resolved.
    Geoip,
    /// Domains, matched once the connection is sniffed.
    Geosite,
}

impl RuleSetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Geoip => "geoip",
            Self::Geosite => "geosite",
        }
    }

    fn contents(self) -> &'static str {
        match self {
            Self::Geoip => "IP networks",
            Self::Geosite => "domains",
        }
    }
}

/// Where a rule-set is downloaded from and what it matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuleSetSource {
    /// `geoip` for a rule-set of IP networks, `geosite` for one of domains.
    pub kind: RuleSetKind,
    /// `https://` address of the binary sing-box rule-set (`.srs`).
    pub url: String,
}

/// Where routed traffic leaves the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Outbound {
    /// Through the VPN tunnel.
    Proxy,
    /// Straight to the network, outside the tunnel.
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

/// Sends the traffic a rule-set matches to an outbound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModeRule {
    /// Tag of a rule-set declared in `rule_sets`.
    pub rule_set: String,
    /// Where the matching traffic goes: `proxy` or `direct`.
    pub route: Outbound,
    /// Whether the mode needs the rule-set: while its file is missing, a
    /// required rule-set sends all traffic to the mode's `final` outbound.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
}

/// A routing mode: its rules, in order, and where the rest of the traffic goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModeDefinition {
    /// Identifier the settings select the mode by, such as `bypass_ru`.
    pub id: String,
    /// Name shown for the mode.
    pub name: String,
    /// Where traffic no rule matches goes: `proxy` or `direct`.
    #[serde(rename = "final")]
    pub final_outbound: Outbound,
    /// Whether private networks (LAN, loopback) stay outside the tunnel.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub private_direct: bool,
    /// Rules, first match wins.
    #[serde(default)]
    pub rules: Vec<ModeRule>,
}

/// Routing definitions, kept in `~/.config/kvn-tui/routing.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = pin_routing_schema_version)]
pub struct RoutingConfig {
    /// JSON Schema an editor checks this file against.
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub json_schema: Option<String>,
    /// Schema version of the file.
    pub schema_version: u32,
    /// Rule-sets by tag. The tag is the name sing-box and DNS rules use and
    /// names the downloaded file, `<tag>.srs`; lowercase letters, digits and
    /// inner hyphens only.
    #[serde(default)]
    pub rule_sets: BTreeMap<String, RuleSetSource>,
    /// Routing modes. The `global` mode is required and sends everything
    /// through the tunnel.
    #[serde(default)]
    pub modes: Vec<ModeDefinition>,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self::with_region_modes(&GeoRegion::ALL)
    }
}

impl RoutingConfig {
    fn with_region_modes(regions: &[GeoRegion]) -> Self {
        let mut config = Self {
            json_schema: None,
            schema_version: ROUTING_SCHEMA_VERSION,
            rule_sets: BTreeMap::new(),
            modes: vec![global_mode()],
        };
        config.insert_all(crate::geo::RULE_SETS);
        for region in regions {
            config.add_region_modes(*region);
        }
        config
    }

    pub fn add_region_modes(&mut self, region: GeoRegion) -> bool {
        let mut extended = self.clone();
        for mode in region_modes(region) {
            if extended.mode(&mode.id).is_none() {
                extended.modes.push(mode);
            }
        }
        let missing: Vec<RuleSet> = crate::geo::RULE_SETS
            .iter()
            .filter(|rule_set| {
                crate::geo::region_rule_set_tags(region).contains(&&*rule_set.tag)
                    && !extended.rule_sets.contains_key(&*rule_set.tag)
            })
            .cloned()
            .collect();
        extended.insert_all(&missing);
        if extended == *self || !extended.diagnostics().is_empty() {
            return false;
        }
        *self = extended;
        true
    }

    pub fn for_new_file(geo_routing: &GeoRouting) -> Self {
        let mut config = Self::with_region_modes(&geo_routing.used_regions());
        for (service, rule_sets) in LEGACY_SERVICE_RULE_SETS {
            if legacy_service_route(geo_routing.legacy_service_routes.as_ref(), service).is_some() {
                config.insert_all(rule_sets);
            }
        }
        config
    }

    pub fn adopt_service_routes(&mut self, legacy_service_routes: &Value) -> bool {
        let before = self.clone();
        let mut rules = Vec::new();
        for (service, rule_sets) in LEGACY_SERVICE_RULE_SETS {
            let Some(route) = legacy_service_route(Some(legacy_service_routes), service) else {
                continue;
            };
            for rule_set in rule_sets {
                if !self.rule_sets.contains_key(&*rule_set.tag) {
                    self.insert_all(std::slice::from_ref(rule_set));
                }
                rules.push(ModeRule {
                    rule_set: rule_set.tag.to_string(),
                    route,
                    required: false,
                });
            }
        }
        for mode in &mut self.modes {
            let missing: Vec<ModeRule> = rules
                .iter()
                .filter(|rule| !mode.rules.iter().any(|own| own.rule_set == rule.rule_set))
                .cloned()
                .collect();
            mode.rules.splice(0..0, missing);
        }
        *self != before
    }

    pub fn mode(&self, id: &str) -> Option<&ModeDefinition> {
        self.modes.iter().find(|mode| mode.id == id)
    }

    pub fn declares(&self, id: &str) -> bool {
        id == GLOBAL_MODE_ID || self.mode(id).is_some()
    }

    pub fn mode_ids(&self) -> Vec<&str> {
        self.modes.iter().map(|mode| mode.id.as_str()).collect()
    }

    pub fn mode_name(&self, id: &str) -> String {
        match self.mode(id) {
            Some(definition) => definition.name.clone(),
            None if id == GLOBAL_MODE_ID => "Global".to_string(),
            None => id.to_string(),
        }
    }

    fn insert_all(&mut self, rule_sets: &[RuleSet]) {
        for rule_set in rule_sets {
            self.rule_sets.insert(
                rule_set.tag.to_string(),
                RuleSetSource {
                    kind: rule_set.kind,
                    url: rule_set.url.to_string(),
                },
            );
        }
    }

    pub(crate) fn rule_set(&self, tag: &str) -> Option<RuleSet> {
        self.rule_sets.get(tag).map(|source| RuleSet {
            tag: Cow::Owned(tag.to_string()),
            kind: source.kind,
            url: Cow::Owned(source.url.clone()),
        })
    }

    pub fn diagnostics(&self) -> Vec<ConfigDiagnostic> {
        let mut diagnostics = Vec::new();
        let mut tags_by_url: HashMap<&str, &str> = HashMap::new();
        for (tag, source) in &self.rule_sets {
            let pointer = format!("/rule_sets/{}", escape_token(tag));
            if !is_valid_tag(tag) {
                diagnostics.push(problem(
                    pointer.clone(),
                    "tag must be lowercase letters, digits and inner hyphens",
                ));
            }
            let url_pointer = format!("{pointer}/url");
            if !is_https_url(&source.url) {
                diagnostics.push(problem(url_pointer.clone(), "must be an https:// address"));
            }
            if let Some(first) = tags_by_url.insert(&source.url, tag) {
                tags_by_url.insert(&source.url, first);
                diagnostics.push(problem(
                    url_pointer,
                    format!("rule set {first} already uses this address"),
                ));
            }
        }
        for builtin in crate::geo::RULE_SETS {
            if let Some(source) = self.rule_sets.get(&*builtin.tag)
                && source.kind != builtin.kind
            {
                diagnostics.push(problem(
                    format!("/rule_sets/{}/kind", escape_token(&builtin.tag)),
                    format!(
                        "must be {}: {} is a list of {}",
                        builtin.kind.as_str(),
                        builtin.tag,
                        builtin.kind.contents()
                    ),
                ));
            }
        }
        diagnostics.extend(self.mode_diagnostics());
        diagnostics
    }

    fn mode_diagnostics(&self) -> Vec<ConfigDiagnostic> {
        let mut diagnostics = Vec::new();
        let mut ids = HashSet::new();
        for (index, mode) in self.modes.iter().enumerate() {
            let pointer = format!("/modes/{index}");
            if !is_valid_mode_id(&mode.id) {
                diagnostics.push(problem(
                    format!("{pointer}/id"),
                    "must be lowercase letters, digits and inner hyphens or underscores",
                ));
            }
            if !ids.insert(mode.id.as_str()) {
                diagnostics.push(problem(
                    format!("{pointer}/id"),
                    format!("mode {} is declared twice", mode.id),
                ));
            }
            if mode.id == GLOBAL_MODE_ID && mode.final_outbound != Outbound::Proxy {
                diagnostics.push(problem(
                    format!("{pointer}/final"),
                    "must be proxy: the global mode sends everything through the tunnel",
                ));
            }
            for (rule_index, rule) in mode.rules.iter().enumerate() {
                if mode.id == GLOBAL_MODE_ID && rule.required {
                    diagnostics.push(problem(
                        format!("{pointer}/rules/{rule_index}/required"),
                        "must be false: the global mode is what other modes fall back to",
                    ));
                }
                if !self.rule_sets.contains_key(&rule.rule_set) {
                    diagnostics.push(problem(
                        format!("{pointer}/rules/{rule_index}/rule_set"),
                        format!("rule set {} is not declared in rule_sets", rule.rule_set),
                    ));
                }
            }
        }
        if self.mode(GLOBAL_MODE_ID).is_none() {
            diagnostics.push(problem(
                "/modes".to_string(),
                "the global mode is missing; it is required",
            ));
        }
        diagnostics
    }
}

pub const GLOBAL_MODE_ID: &str = "global";

fn global_mode() -> ModeDefinition {
    ModeDefinition {
        id: GLOBAL_MODE_ID.to_string(),
        name: "Global".to_string(),
        final_outbound: Outbound::Proxy,
        private_direct: false,
        rules: Vec::new(),
    }
}

fn region_modes(region: GeoRegion) -> Vec<ModeDefinition> {
    let tags = crate::geo::region_rule_set_tags(region);
    if tags.is_empty() {
        return Vec::new();
    }
    let mode = |kind: &str, name: &str, region_outbound: Outbound, final_outbound: Outbound| {
        ModeDefinition {
            id: format!("{kind}_{}", region.as_str()),
            name: format!("{name} {}", region.code_upper()),
            final_outbound,
            private_direct: true,
            rules: tags
                .iter()
                .map(|tag| ModeRule {
                    rule_set: tag.to_string(),
                    route: region_outbound,
                    required: true,
                })
                .collect(),
        }
    };
    vec![
        mode("bypass", "Bypass", Outbound::Direct, Outbound::Proxy),
        mode("only", "Only", Outbound::Proxy, Outbound::Direct),
    ]
}

fn is_valid_mode_id(id: &str) -> bool {
    !id.is_empty()
        && id.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id.ends_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn problem(pointer: String, message: impl std::fmt::Display) -> ConfigDiagnostic {
    let message = format!("{pointer}: {message}");
    ConfigDiagnostic::new(pointer, message)
}

fn legacy_service_route(legacy_service_routes: Option<&Value>, service: &str) -> Option<Outbound> {
    match legacy_service_routes?.get(service)?.as_str()? {
        "proxy" => Some(Outbound::Proxy),
        "direct" => Some(Outbound::Direct),
        _ => None,
    }
}

fn is_valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && !tag.starts_with('-')
        && !tag.ends_with('-')
        && tag
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn is_https_url(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|url| url.scheme() == "https" && url.host().is_some())
}

fn pin_routing_schema_version(schema: &mut schemars::Schema) {
    if let Some(version) = schema
        .get_mut("properties")
        .and_then(|properties| properties.get_mut("schema_version"))
        .and_then(Value::as_object_mut)
    {
        version.insert("const".into(), ROUTING_SCHEMA_VERSION.into());
    }
}

pub fn routing_json_schema() -> Value {
    schemars::generate::SchemaSettings::draft07()
        .into_generator()
        .into_root_schema_for::<RoutingConfig>()
        .to_value()
}

pub fn load_routing_at(
    path: &Path,
    geo_routing: &GeoRouting,
    create_missing: bool,
) -> Result<RoutingConfig> {
    match fs::read_to_string(path) {
        Ok(contents) => parse_routing(&contents, path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let config = RoutingConfig::for_new_file(geo_routing);
            if create_missing {
                create_routing_at(path, &config)?;
            }
            Ok(config)
        }
        Err(error) => Err(error).with_context(|| format!("Failed to read {path:?}")),
    }
}

fn parse_routing(contents: &str, path: &Path) -> Result<RoutingConfig> {
    let document: Value =
        serde_json::from_str(contents).with_context(|| format!("Failed to parse {path:?}"))?;
    if let Some(version) = document.get("schema_version").and_then(Value::as_u64)
        && version > u64::from(ROUTING_SCHEMA_VERSION)
    {
        anyhow::bail!(
            "{path:?} uses schema_version {version}, newer than this build supports (max {ROUTING_SCHEMA_VERSION}); upgrade kvn"
        );
    }
    into_result(schema_diagnostics_against(&SCHEMA, &document))
        .with_context(|| format!("Invalid {path:?}"))?;
    let config: RoutingConfig =
        serde_json::from_value(document).with_context(|| format!("Failed to parse {path:?}"))?;
    into_result(config.diagnostics()).with_context(|| format!("Invalid {path:?}"))?;
    Ok(config)
}

pub(crate) fn update_routing_at(
    path: &Path,
    change: impl FnOnce(&mut RoutingConfig) -> bool,
) -> Result<RoutingConfig> {
    let contents = fs::read_to_string(path).with_context(|| format!("Failed to read {path:?}"))?;
    let mut config = parse_routing(&contents, path)?;
    if change(&mut config) {
        let json = serde_json::to_string_pretty(&config)?;
        crate::atomic_write::write_if_unchanged(path, json.as_bytes(), Some(contents.as_bytes()))
            .with_context(|| format!("Failed to update {path:?}"))?;
    }
    Ok(config)
}

fn create_routing_at(path: &Path, config: &RoutingConfig) -> Result<()> {
    let dir = path.parent().context("Invalid routing config path")?;
    fs::create_dir_all(dir).with_context(|| format!("Failed to create {dir:?}"))?;
    let json = serde_json::to_string_pretty(config)?;
    crate::atomic_write::write_if_unchanged(path, json.as_bytes(), None)
        .with_context(|| format!("Failed to create {path:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pointers(config: &RoutingConfig) -> Vec<String> {
        config
            .diagnostics()
            .into_iter()
            .map(|diagnostic| diagnostic.pointer)
            .collect()
    }

    fn custom(url: &str) -> RuleSetSource {
        RuleSetSource {
            kind: RuleSetKind::Geosite,
            url: url.to_string(),
        }
    }

    fn geo_routing(regions: &[GeoRegion], legacy: Option<Value>) -> GeoRouting {
        GeoRouting {
            legacy_current_region: regions.first().copied(),
            legacy_selected_region_modes: regions
                .iter()
                .map(|region| (*region, GLOBAL_MODE_ID.to_string()))
                .collect(),
            legacy_service_routes: legacy,
            ..GeoRouting::default()
        }
    }

    fn mode_ids(config: &RoutingConfig) -> Vec<&str> {
        config.modes.iter().map(|mode| mode.id.as_str()).collect()
    }

    #[test]
    fn a_new_file_holds_the_builtin_rule_sets_and_passes_its_own_checks() {
        let config = RoutingConfig::for_new_file(&GeoRouting::default());

        let tags: Vec<&str> = config.rule_sets.keys().map(String::as_str).collect();
        let mut builtin: Vec<&str> = crate::geo::RULE_SETS
            .iter()
            .map(|rule_set| &*rule_set.tag)
            .collect();
        builtin.sort_unstable();
        assert_eq!(tags, builtin);
        assert_eq!(
            config.rule_set("geoip-ru"),
            crate::geo::RULE_SETS
                .iter()
                .find(|rule_set| rule_set.tag == "geoip-ru")
                .cloned()
        );
        assert!(config.diagnostics().is_empty());
        let document = serde_json::to_value(&config).unwrap();
        assert!(schema_diagnostics_against(&SCHEMA, &document).is_empty());
    }

    #[test]
    fn a_new_file_holds_global_and_the_modes_of_the_regions_in_use() {
        assert_eq!(
            mode_ids(&RoutingConfig::for_new_file(&GeoRouting::default())),
            ["global"]
        );

        let config =
            RoutingConfig::for_new_file(&geo_routing(&[GeoRegion::Cn, GeoRegion::Ru], None));

        assert_eq!(
            mode_ids(&config),
            ["global", "bypass_ru", "only_ru", "bypass_cn", "only_cn"]
        );
        assert!(config.diagnostics().is_empty());
    }

    #[test]
    fn default_modes_route_their_region_as_before() {
        let config = RoutingConfig::default();

        let global = config.mode("global").unwrap();
        assert_eq!(global.name, "Global");
        assert_eq!(global.final_outbound, Outbound::Proxy);
        assert!(!global.private_direct);
        assert!(global.rules.is_empty());

        let only_ir = config.mode("only_ir").unwrap();
        assert_eq!(only_ir.name, "Only IR");
        assert_eq!(only_ir.final_outbound, Outbound::Direct);
        assert!(only_ir.private_direct);
        assert_eq!(
            only_ir.rules,
            ["geosite-category-ir", "geoip-ir"].map(|tag| ModeRule {
                rule_set: tag.to_string(),
                route: Outbound::Proxy,
                required: true,
            })
        );
        assert_eq!(
            config.mode("bypass_cn").unwrap().rules[0].route,
            Outbound::Direct
        );
    }

    #[test]
    fn a_new_file_keeps_the_enabled_service_overrides_as_rule_sets() {
        let legacy = json!({ "steam": "direct", "telegram": "disabled" });

        let config = RoutingConfig::for_new_file(&geo_routing(&[], Some(legacy)));

        let steam = [
            (
                "geosite-steam",
                RuleSetKind::Geosite,
                "https://raw.githubusercontent.com/MetaCubeX/meta-rules-dat/sing/geo/geosite/steam.srs",
            ),
            (
                "geoip-steam",
                RuleSetKind::Geoip,
                "https://raw.githubusercontent.com/MetaCubeX/meta-rules-dat/sing/asn/AS32590.srs",
            ),
        ];
        for (tag, kind, url) in steam {
            let rule_set = config.rule_set(tag).unwrap();
            assert_eq!((rule_set.kind, &*rule_set.url), (kind, url));
            assert_eq!(rule_set.key(), format!("{tag}.srs"));
        }
        assert!(config.rule_set("geosite-telegram").is_none());

        let both = RoutingConfig::for_new_file(&geo_routing(
            &[],
            Some(json!({ "steam": "proxy", "telegram": "direct" })),
        ));
        for tag in ["geosite-telegram", "geoip-telegram"] {
            assert!(both.rule_set(tag).is_some(), "{tag}");
        }
        assert!(both.diagnostics().is_empty());
    }

    #[test]
    fn service_routes_become_the_first_rules_of_every_mode() {
        let legacy = json!({ "steam": "direct", "telegram": "proxy" });
        let mut config = RoutingConfig::for_new_file(&geo_routing(&[GeoRegion::Ru], None));
        config.rule_sets.insert(
            "geoip-telegram".into(),
            RuleSetSource {
                kind: RuleSetKind::Geoip,
                url: "https://mirror.example/telegram.srs".into(),
            },
        );

        assert!(config.adopt_service_routes(&legacy));

        let expected: Vec<(&str, Outbound)> = vec![
            ("geosite-steam", Outbound::Direct),
            ("geoip-steam", Outbound::Direct),
            ("geosite-telegram", Outbound::Proxy),
            ("geoip-telegram", Outbound::Proxy),
        ];
        for mode in &config.modes {
            let leading: Vec<(&str, Outbound)> = mode.rules[..4]
                .iter()
                .map(|rule| (rule.rule_set.as_str(), rule.route))
                .collect();
            assert_eq!(leading, expected, "{}", mode.id);
            assert!(mode.rules[..4].iter().all(|rule| !rule.required));
        }
        assert_eq!(
            config.mode("bypass_ru").unwrap().rules[4].rule_set,
            "geosite-category-ru"
        );
        assert_eq!(
            config.rule_sets["geoip-telegram"].url,
            "https://mirror.example/telegram.srs"
        );
        assert!(config.diagnostics().is_empty());
        assert!(!config.adopt_service_routes(&legacy));
    }

    #[test]
    fn disabled_service_routes_add_nothing() {
        let mut config = RoutingConfig::default();

        assert!(!config.adopt_service_routes(&json!({ "steam": "disabled" })));
        assert!(!config.adopt_service_routes(&json!({})));
    }

    #[test]
    fn diagnostics_point_at_the_offending_rule_set() {
        let mut config = RoutingConfig::default();
        config
            .rule_sets
            .insert("Ads/List".into(), custom("https://example.com/ads.srs"));
        config
            .rule_sets
            .insert("plain".into(), custom("http://example.com/plain.srs"));
        config
            .rule_sets
            .insert("zz-copy".into(), custom("https://example.com/ads.srs"));
        config.rule_sets.get_mut("geoip-ir").unwrap().kind = RuleSetKind::Geosite;

        assert_eq!(
            pointers(&config),
            [
                "/rule_sets/Ads~1List",
                "/rule_sets/plain/url",
                "/rule_sets/zz-copy/url",
                "/rule_sets/geoip-ir/kind",
            ]
        );
        let messages: Vec<String> = config
            .diagnostics()
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect();
        assert!(
            messages[2].contains("rule set Ads/List already uses"),
            "{messages:?}"
        );
        assert!(
            messages[3].contains("must be geoip: geoip-ir is a list of IP networks"),
            "{messages:?}"
        );
    }

    #[test]
    fn diagnostics_point_at_the_offending_mode() {
        let mut config = RoutingConfig::default();
        config.rule_sets.remove("geoip-cn");
        let global = config
            .modes
            .iter_mut()
            .find(|mode| mode.id == "global")
            .unwrap();
        global.final_outbound = Outbound::Direct;
        let mut copy = config.mode("only_ir").unwrap().clone();
        copy.id = "Only IR".into();
        config.modes.push(copy.clone());
        copy.id = "only_ir".into();
        config.modes.push(copy);

        let cn = config
            .modes
            .iter()
            .position(|mode| mode.id == "bypass_cn")
            .unwrap();
        let only_cn = config
            .modes
            .iter()
            .position(|mode| mode.id == "only_cn")
            .unwrap();
        let last = config.modes.len() - 1;
        assert_eq!(
            pointers(&config),
            [
                "/modes/0/final".to_string(),
                format!("/modes/{cn}/rules/1/rule_set"),
                format!("/modes/{only_cn}/rules/1/rule_set"),
                format!("/modes/{}/id", last - 1),
                format!("/modes/{last}/id"),
            ]
        );

        config.modes.retain(|mode| mode.id != "global");
        assert!(pointers(&config).contains(&"/modes".to_string()));

        let mut config = RoutingConfig::default();
        config.modes[0].rules.push(ModeRule {
            rule_set: "geoip-ru".into(),
            route: Outbound::Direct,
            required: true,
        });
        assert_eq!(pointers(&config), ["/modes/0/rules/0/required"]);
    }

    #[test]
    fn tags_are_lowercase_words_joined_by_hyphens() {
        for tag in ["ads", "geoip-steam", "list-2"] {
            assert!(is_valid_tag(tag), "{tag}");
        }
        for tag in ["", "-ads", "ads-", "Ads", "ads_list", "ads.srs", "../ads"] {
            assert!(!is_valid_tag(tag), "{tag}");
        }
    }

    #[test]
    fn mode_ids_are_lowercase_words_joined_by_hyphens_or_underscores() {
        for id in ["global", "bypass_ru", "work-2"] {
            assert!(is_valid_mode_id(id), "{id}");
        }
        for id in ["", "_work", "work-", "Work", "work mode"] {
            assert!(!is_valid_mode_id(id), "{id}");
        }
    }

    #[test]
    fn a_missing_file_is_created_only_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kvn-tui").join("routing.json");
        let in_use = geo_routing(&[GeoRegion::Ir], Some(json!({ "telegram": "proxy" })));

        let shown = load_routing_at(&path, &in_use, false).unwrap();
        assert!(!path.exists());

        let created = load_routing_at(&path, &in_use, true).unwrap();
        assert_eq!(created, shown);
        assert!(created.rule_set("geoip-telegram").is_some());
        assert!(created.mode("only_ir").is_some());
        assert_eq!(
            load_routing_at(&path, &GeoRouting::default(), true).unwrap(),
            created
        );
    }

    #[test]
    fn an_existing_file_is_read_as_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("routing.json");
        let mut config = RoutingConfig::default();
        config
            .rule_sets
            .insert("ads".into(), custom("https://example.com/ads.srs"));
        fs::write(&path, serde_json::to_string(&config).unwrap()).unwrap();

        assert_eq!(
            load_routing_at(&path, &geo_routing(&[GeoRegion::Ru], None), true).unwrap(),
            config
        );
    }

    #[test]
    fn an_invalid_file_is_refused_with_the_offending_pointer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("routing.json");
        let mut document = serde_json::to_value(RoutingConfig::default()).unwrap();
        document["rule_sets"]["ads"] = json!({ "kind": "geosite", "uri": "https://x/ads.srs" });
        fs::write(&path, document.to_string()).unwrap();
        let schema_error = format!(
            "{:#}",
            load_routing_at(&path, &GeoRouting::default(), true).unwrap_err()
        );
        assert!(schema_error.contains("/rule_sets/ads"), "{schema_error}");

        document["rule_sets"]["ads"] = json!({ "kind": "geosite", "url": "http://x/ads.srs" });
        fs::write(&path, document.to_string()).unwrap();
        let check_error = format!(
            "{:#}",
            load_routing_at(&path, &GeoRouting::default(), true).unwrap_err()
        );
        assert!(
            check_error.contains("/rule_sets/ads/url: must be an https:// address"),
            "{check_error}"
        );

        document["schema_version"] = json!(ROUTING_SCHEMA_VERSION + 1);
        fs::write(&path, document.to_string()).unwrap();
        let version_error = format!(
            "{:#}",
            load_routing_at(&path, &GeoRouting::default(), true).unwrap_err()
        );
        assert!(version_error.contains("upgrade kvn"), "{version_error}");
    }

    #[test]
    fn region_modes_are_added_once_with_their_rule_sets() {
        let mut config = RoutingConfig::for_new_file(&GeoRouting::default());
        config.rule_sets.remove("geoip-cn");

        assert!(config.add_region_modes(GeoRegion::Cn));
        assert_eq!(mode_ids(&config), ["global", "bypass_cn", "only_cn"]);
        assert_eq!(
            config.rule_set("geoip-cn"),
            crate::geo::RULE_SETS
                .iter()
                .find(|rule_set| rule_set.tag == "geoip-cn")
                .cloned()
        );
        assert!(!config.add_region_modes(GeoRegion::Cn));
        assert!(!config.add_region_modes(GeoRegion::Global));
    }

    #[test]
    fn region_modes_that_would_break_the_file_are_not_added() {
        let mut config = RoutingConfig::for_new_file(&GeoRouting::default());
        config.rule_sets.remove("geoip-ir");
        config
            .rule_sets
            .insert("mirror".into(), custom(&crate::geo::RULE_SETS[4].url));
        let before = config.clone();

        assert!(!config.add_region_modes(GeoRegion::Ir));
        assert_eq!(config, before);
    }

    #[test]
    fn modes_are_listed_and_named_from_the_file() {
        let mut config = RoutingConfig::for_new_file(&GeoRouting::default());
        config.modes[0].name = "Everything".into();
        config.add_region_modes(GeoRegion::Ru);

        assert_eq!(config.mode_ids(), ["global", "bypass_ru", "only_ru"]);
        assert_eq!(config.mode_name("global"), "Everything");
        assert_eq!(config.mode_name("only_ru"), "Only RU");
        assert_eq!(config.mode_name("work"), "work");
        assert!(config.declares("bypass_ru"));
        assert!(!config.declares("work"));

        config.modes.clear();
        assert!(config.declares("global"));
        assert_eq!(config.mode_name("global"), "Global");
    }
}
