use std::collections::{BTreeSet, HashMap};

use anyhow::Context;
use serde_json::Value;

use super::profile::Config;
use super::routing::RoutingConfig;

pub(crate) mod resolution;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MergeError {
    Conflicts(Vec<String>),
    Invalid(String),
}

impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflicts(paths) => write!(f, "config conflicts at {}", paths.join(", ")),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for MergeError {}

pub(crate) fn merge_configs(
    base: &Config,
    current: &Config,
    edited: &Config,
) -> Result<Config, MergeError> {
    for (label, config) in [("base", base), ("current", current), ("edited", edited)] {
        if let Err(error) = config.validate() {
            return Err(MergeError::Invalid(format!(
                "{label} config is invalid: {error}"
            )));
        }
    }
    let base = serde_json::to_value(base).expect("Config serialization cannot fail");
    let current = serde_json::to_value(current).expect("Config serialization cannot fail");
    let edited = serde_json::to_value(edited).expect("Config serialization cannot fail");
    let mut conflicts = Vec::new();
    let merged = merge_value(
        Some(&base),
        Some(&current),
        Some(&edited),
        "",
        &mut conflicts,
        false,
    );
    if !conflicts.is_empty() {
        return Err(MergeError::Conflicts(conflicts));
    }
    let config: Config = serde_json::from_value(merged.expect("root config cannot be deleted"))
        .context("merged config is invalid")
        .map_err(|error| MergeError::Invalid(error.to_string()))?;
    let mut config = config;
    if config
        .settings
        .last_connected_profile
        .is_some_and(|id| !config.profiles.iter().any(|profile| profile.id == id))
    {
        config.settings.last_connected_profile = None;
    }
    config
        .validate()
        .context("merged config failed validation")
        .map_err(|error| MergeError::Invalid(error.to_string()))?;
    Ok(config)
}

pub(crate) fn merge_routing(
    base: &RoutingConfig,
    current: &RoutingConfig,
    edited: &RoutingConfig,
) -> Result<RoutingConfig, MergeError> {
    for (label, routing) in [("base", base), ("current", current), ("edited", edited)] {
        if let Some(problem) = routing.diagnostics().first() {
            return Err(MergeError::Invalid(format!(
                "{label} routing configuration is invalid: {}",
                problem.message
            )));
        }
    }
    let [base, current, edited] = [base, current, edited].map(|routing| {
        serde_json::to_value(routing).expect("RoutingConfig serialization cannot fail")
    });
    let mut conflicts = Vec::new();
    let merged = merge_value(
        Some(&base),
        Some(&current),
        Some(&edited),
        "",
        &mut conflicts,
        false,
    );
    if !conflicts.is_empty() {
        return Err(MergeError::Conflicts(conflicts));
    }
    let routing: RoutingConfig =
        serde_json::from_value(merged.expect("root routing configuration cannot be deleted"))
            .context("merged routing configuration is invalid")
            .map_err(|error| MergeError::Invalid(format!("{error:#}")))?;
    if let Some(problem) = routing.diagnostics().first() {
        return Err(MergeError::Invalid(format!(
            "merged routing configuration is invalid: {}",
            problem.message
        )));
    }
    Ok(routing)
}

fn merge_value(
    base: Option<&Value>,
    current: Option<&Value>,
    edited: Option<&Value>,
    path: &str,
    conflicts: &mut Vec<String>,
    prefer_edited: bool,
) -> Option<Value> {
    if edited == base {
        return current.cloned();
    }
    if current == base {
        return edited.cloned();
    }
    if current == edited {
        return current.cloned();
    }

    match (base, current, edited) {
        (Some(Value::Object(b)), Some(Value::Object(c)), Some(Value::Object(e))) => {
            let keys: BTreeSet<_> = b.keys().chain(c.keys()).chain(e.keys()).collect();
            let mut out = serde_json::Map::new();
            for key in keys {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                if let Some(value) = merge_value(
                    b.get(key),
                    c.get(key),
                    e.get(key),
                    &child_path,
                    conflicts,
                    prefer_edited,
                ) {
                    out.insert(key.clone(), value);
                }
            }
            Some(Value::Object(out))
        }
        (Some(Value::Array(b)), Some(Value::Array(c)), Some(Value::Array(e)))
            if matches!(path, "profiles" | "subscriptions" | "modes") =>
        {
            merge_uuid_array(b, c, e, path, conflicts, prefer_edited)
        }
        _ => {
            conflicts.push(if path.is_empty() {
                "configuration".into()
            } else {
                path.into()
            });
            if prefer_edited {
                edited.cloned()
            } else {
                current.cloned()
            }
        }
    }
}

fn merge_uuid_array(
    base: &[Value],
    current: &[Value],
    edited: &[Value],
    path: &str,
    conflicts: &mut Vec<String>,
    prefer_edited: bool,
) -> Option<Value> {
    fn index(values: &[Value]) -> HashMap<String, &Value> {
        values
            .iter()
            .filter_map(|value| {
                value
                    .get("id")
                    .and_then(Value::as_str)
                    .map(|id| (id.to_owned(), value))
            })
            .collect()
    }
    let b = index(base);
    let c = index(current);
    let e = index(edited);
    let id_order = |values: &[Value]| {
        values
            .iter()
            .filter_map(|value| value.get("id").and_then(Value::as_str).map(str::to_owned))
            .collect::<Vec<_>>()
    };
    let base_order = id_order(base);
    let current_order = id_order(current);
    let edited_order = id_order(edited);
    let current_reordered = common_order_changed(&base_order, &current_order);
    let edited_reordered = common_order_changed(&base_order, &edited_order);
    let order_conflict = current_reordered && edited_reordered && current_order != edited_order;
    if order_conflict {
        conflicts.push(format!("{path} order"));
    }
    let mut ids = Vec::new();
    let use_edited_order = edited_reordered && (!order_conflict || prefer_edited);
    let first = if use_edited_order { edited } else { current };
    let second = if use_edited_order { current } else { edited };
    for value in first.iter().chain(second) {
        if let Some(id) = value.get("id").and_then(Value::as_str)
            && !ids.iter().any(|known| known == id)
        {
            ids.push(id.to_owned());
        }
    }
    let mut out = Vec::new();
    for id in ids {
        if let Some(value) = merge_value(
            b.get(&id).copied(),
            c.get(&id).copied(),
            e.get(&id).copied(),
            &format!("{path}[{id}]"),
            conflicts,
            prefer_edited,
        ) {
            out.push(value);
        }
    }
    Some(Value::Array(out))
}

fn common_order_changed(base: &[String], side: &[String]) -> bool {
    let common: Vec<_> = base.iter().filter(|id| side.contains(id)).collect();
    let side_common: Vec<_> = side.iter().filter(|id| base.contains(id)).collect();
    common != side_common
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::profile::Profile;

    #[test]
    fn merges_disjoint_profile_and_setting_changes() {
        let mut base = Config::default();
        base.profiles.push(Profile::new_vless(
            "A".into(),
            "1.1.1.1".into(),
            443,
            uuid::Uuid::new_v4().to_string(),
        ));
        let mut current = base.clone();
        current.settings.auto_connect = true;
        let mut edited = base.clone();
        edited.profiles[0].name = "Edited".into();
        let merged = merge_configs(&base, &current, &edited).unwrap();
        assert!(merged.settings.auto_connect);
        assert_eq!(merged.profiles[0].name, "Edited");
    }

    #[test]
    fn reports_same_field_conflict() {
        let base = Config::default();
        let mut current = base.clone();
        current.settings.theme = "nord".into();
        let mut edited = base.clone();
        edited.settings.theme = "catppuccin".into();
        assert_eq!(
            merge_configs(&base, &current, &edited).unwrap_err(),
            MergeError::Conflicts(vec!["settings.theme".into()])
        );
    }

    #[test]
    fn merges_independent_additions_and_deletions_by_uuid() {
        let mut base = Config::default();
        let removed = Profile::new_vless(
            "Removed".into(),
            "1.1.1.1".into(),
            443,
            uuid::Uuid::new_v4().to_string(),
        );
        base.profiles.push(removed);
        let mut current = base.clone();
        current.profiles.push(Profile::new_vless(
            "Daemon".into(),
            "2.2.2.2".into(),
            443,
            uuid::Uuid::new_v4().to_string(),
        ));
        let mut edited = base.clone();
        edited.profiles.clear();
        edited.profiles.push(Profile::new_vless(
            "Editor".into(),
            "3.3.3.3".into(),
            443,
            uuid::Uuid::new_v4().to_string(),
        ));

        let merged = merge_configs(&base, &current, &edited).unwrap();
        let names: Vec<_> = merged
            .profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect();
        assert_eq!(names, vec!["Daemon", "Editor"]);
    }

    #[test]
    fn modification_conflicts_with_concurrent_deletion() {
        let mut base = Config::default();
        base.profiles.push(Profile::new_vless(
            "A".into(),
            "1.1.1.1".into(),
            443,
            uuid::Uuid::new_v4().to_string(),
        ));
        let mut current = base.clone();
        current.profiles.clear();
        let mut edited = base.clone();
        edited.profiles[0].name = "Edited".into();

        let conflicts = merge_configs(&base, &current, &edited).unwrap_err();
        assert!(
            matches!(conflicts, MergeError::Conflicts(paths) if paths[0].starts_with("profiles["))
        );
    }

    #[test]
    fn rejects_duplicate_ids_before_indexing() {
        let mut base = Config::default();
        let profile = Profile::new_vless(
            "A".into(),
            "1.1.1.1".into(),
            443,
            uuid::Uuid::new_v4().to_string(),
        );
        base.profiles = vec![profile.clone(), profile];

        let MergeError::Invalid(message) =
            merge_configs(&base, &Config::default(), &Config::default()).unwrap_err()
        else {
            panic!("expected validation error")
        };
        assert!(message.contains("base config is invalid"));
        assert!(message.contains("duplicate id"));
    }

    #[test]
    fn preserves_editor_reorder_during_concurrent_field_change() {
        let mut base = Config::default();
        let a = Profile::new_vless(
            "A".into(),
            "1.1.1.1".into(),
            443,
            uuid::Uuid::new_v4().to_string(),
        );
        let b = Profile::new_vless(
            "B".into(),
            "2.2.2.2".into(),
            443,
            uuid::Uuid::new_v4().to_string(),
        );
        base.profiles = vec![a, b];
        let mut current = base.clone();
        current.settings.auto_connect = true;
        let mut edited = base.clone();
        edited.profiles.reverse();
        let merged = merge_configs(&base, &current, &edited).unwrap();
        assert_eq!(merged.profiles[0].name, "B");
        assert!(merged.settings.auto_connect);
    }

    #[test]
    fn conflicting_reorders_are_reported() {
        let mut base = Config::default();
        for name in ["A", "B", "C"] {
            base.profiles.push(Profile::new_vless(
                name.into(),
                "1.1.1.1".into(),
                443,
                uuid::Uuid::new_v4().to_string(),
            ));
        }
        let mut current = base.clone();
        current.profiles.swap(0, 1);
        let mut edited = base.clone();
        edited.profiles.swap(1, 2);
        assert!(
            matches!(merge_configs(&base, &current, &edited), Err(MergeError::Conflicts(paths)) if paths.contains(&"profiles order".into()))
        );
    }

    #[test]
    fn clears_dangling_last_connected_profile_after_merge() {
        let mut base = Config::default();
        let profile = Profile::new_vless(
            "A".into(),
            "1.1.1.1".into(),
            443,
            uuid::Uuid::new_v4().to_string(),
        );
        base.settings.last_connected_profile = Some(profile.id);
        base.profiles.push(profile);
        let mut edited = base.clone();
        edited.profiles.clear();
        let merged = merge_configs(&base, &base, &edited).unwrap();
        assert_eq!(merged.settings.last_connected_profile, None);
    }

    fn routing_mode<'a>(
        routing: &'a mut RoutingConfig,
        id: &str,
    ) -> &'a mut crate::config::routing::ModeDefinition {
        routing.modes.iter_mut().find(|mode| mode.id == id).unwrap()
    }

    #[test]
    fn merges_routing_modes_by_id_and_rule_sets_by_tag() {
        let base = RoutingConfig::default();
        let mut current = base.clone();
        routing_mode(&mut current, "bypass_ru").name = "Russia".into();
        let mut work = base.rule_sets["geoip-ru"].clone();
        work.url = "https://example.com/work.srs".into();
        current.rule_sets.insert("work".into(), work);
        let mut edited = base.clone();
        routing_mode(&mut edited, "only_cn").name = "China".into();
        edited
            .modes
            .retain(|mode| mode.id != "bypass_ir" && mode.id != "only_ir");
        edited.rule_sets.remove("geoip-ir");

        let merged = merge_routing(&base, &current, &edited).unwrap();

        assert_eq!(merged.mode("bypass_ru").unwrap().name, "Russia");
        assert_eq!(merged.mode("only_cn").unwrap().name, "China");
        assert!(merged.mode("bypass_ir").is_none());
        assert!(merged.rule_sets.contains_key("work"));
        assert!(!merged.rule_sets.contains_key("geoip-ir"));
    }

    #[test]
    fn rules_of_a_mode_changed_on_both_sides_conflict_as_a_whole() {
        let base = RoutingConfig::default();
        let mut current = base.clone();
        routing_mode(&mut current, "bypass_ru").rules.pop();
        let mut edited = base.clone();
        routing_mode(&mut edited, "bypass_ru").rules.remove(0);

        assert_eq!(
            merge_routing(&base, &current, &edited).unwrap_err(),
            MergeError::Conflicts(vec!["modes[bypass_ru].rules".into()])
        );
    }

    #[test]
    fn a_merge_that_breaks_routing_checks_is_refused() {
        let base = RoutingConfig::default();
        let mut current = base.clone();
        current.rule_sets.remove("geosite-cn");
        current
            .modes
            .retain(|mode| mode.id != "bypass_cn" && mode.id != "only_cn");
        let mut edited = base.clone();
        routing_mode(&mut edited, "global")
            .rules
            .push(crate::config::routing::ModeRule::rule_set(
                "geosite-cn",
                crate::config::routing::Outbound::Direct,
                false,
            ));

        assert!(matches!(
            merge_routing(&base, &current, &edited),
            Err(MergeError::Invalid(message)) if message.contains("geosite-cn")
        ));
    }
}
