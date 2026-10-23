use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::json_pointer::escape_token;
use super::profile::{ConfigDiagnostic, GeoRegion, into_result, schema_diagnostics_against};
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
}

impl Default for RoutingConfig {
    fn default() -> Self {
        let mut config = Self {
            json_schema: None,
            schema_version: ROUTING_SCHEMA_VERSION,
            rule_sets: BTreeMap::new(),
        };
        config.insert_all(crate::geo::RULE_SETS);
        config
    }
}

impl RoutingConfig {
    pub fn for_new_file(legacy_service_routes: Option<&Value>) -> Self {
        let mut config = Self::default();
        for (service, rule_sets) in LEGACY_SERVICE_RULE_SETS {
            if legacy_service_enabled(legacy_service_routes, service) {
                config.insert_all(rule_sets);
            }
        }
        config
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
        for region in GeoRegion::ALL {
            for tag in crate::geo::region_rule_set_tags(region) {
                let modes = region.code_upper();
                let Some(source) = self.rule_sets.get(*tag) else {
                    diagnostics.push(problem(
                        "/rule_sets".to_string(),
                        format!("rule set {tag} is missing; the {modes} routing modes need it"),
                    ));
                    continue;
                };
                if let Some(expected) = builtin_kind(tag)
                    && source.kind != expected
                {
                    diagnostics.push(problem(
                        format!("/rule_sets/{}/kind", escape_token(tag)),
                        format!(
                            "must be {}: the {modes} routing modes route it as one",
                            expected.as_str()
                        ),
                    ));
                }
            }
        }
        diagnostics
    }
}

fn builtin_kind(tag: &str) -> Option<RuleSetKind> {
    crate::geo::RULE_SETS
        .iter()
        .find(|rule_set| rule_set.tag == tag)
        .map(|rule_set| rule_set.kind)
}

fn problem(pointer: String, message: impl std::fmt::Display) -> ConfigDiagnostic {
    let message = format!("{pointer}: {message}");
    ConfigDiagnostic::new(pointer, message)
}

fn legacy_service_enabled(legacy_service_routes: Option<&Value>, service: &str) -> bool {
    legacy_service_routes
        .and_then(|routes| routes.get(service))
        .and_then(Value::as_str)
        .is_some_and(|route| matches!(route, "proxy" | "direct"))
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
    legacy_service_routes: Option<&Value>,
    create_missing: bool,
) -> Result<RoutingConfig> {
    match fs::read_to_string(path) {
        Ok(contents) => parse_routing(&contents, path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let config = RoutingConfig::for_new_file(legacy_service_routes);
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

    #[test]
    fn a_new_file_holds_the_builtin_rule_sets_and_passes_its_own_checks() {
        let config = RoutingConfig::for_new_file(None);

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
    fn a_new_file_keeps_the_enabled_service_overrides_as_rule_sets() {
        let legacy = json!({ "steam": "direct", "telegram": "disabled" });

        let config = RoutingConfig::for_new_file(Some(&legacy));

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

        let both = RoutingConfig::for_new_file(Some(&json!({
            "steam": "proxy",
            "telegram": "direct"
        })));
        for tag in ["geosite-telegram", "geoip-telegram"] {
            assert!(both.rule_set(tag).is_some(), "{tag}");
        }
        assert!(both.diagnostics().is_empty());
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
        config.rule_sets.remove("geoip-cn");
        config.rule_sets.get_mut("geoip-ir").unwrap().kind = RuleSetKind::Geosite;

        assert_eq!(
            pointers(&config),
            [
                "/rule_sets/Ads~1List",
                "/rule_sets/plain/url",
                "/rule_sets/zz-copy/url",
                "/rule_sets",
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
            messages[4].contains("must be geoip: the IR routing modes"),
            "{messages:?}"
        );
        assert!(
            messages[3].contains("geoip-cn is missing; the CN routing modes"),
            "{messages:?}"
        );
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
    fn a_missing_file_is_created_only_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kvn").join("routing.json");
        let legacy = json!({ "telegram": "proxy" });

        let shown = load_routing_at(&path, Some(&legacy), false).unwrap();
        assert!(!path.exists());

        let created = load_routing_at(&path, Some(&legacy), true).unwrap();
        assert_eq!(created, shown);
        assert!(created.rule_set("geoip-telegram").is_some());
        assert_eq!(load_routing_at(&path, None, true).unwrap(), created);
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
            load_routing_at(&path, Some(&json!({ "steam": "direct" })), true).unwrap(),
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
        let schema_error = format!("{:#}", load_routing_at(&path, None, true).unwrap_err());
        assert!(schema_error.contains("/rule_sets/ads"), "{schema_error}");

        document["rule_sets"]["ads"] = json!({ "kind": "geosite", "url": "http://x/ads.srs" });
        fs::write(&path, document.to_string()).unwrap();
        let check_error = format!("{:#}", load_routing_at(&path, None, true).unwrap_err());
        assert!(
            check_error.contains("/rule_sets/ads/url: must be an https:// address"),
            "{check_error}"
        );

        document["schema_version"] = json!(ROUTING_SCHEMA_VERSION + 1);
        fs::write(&path, document.to_string()).unwrap();
        let version_error = format!("{:#}", load_routing_at(&path, None, true).unwrap_err());
        assert!(version_error.contains("upgrade kvn"), "{version_error}");
    }
}
