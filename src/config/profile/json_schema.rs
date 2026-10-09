use std::sync::LazyLock;

use serde_json::{Map, Value};

use super::{CURRENT_SCHEMA_VERSION, Config, ConfigDiagnostic, ProtocolConfig};

static SCHEMA: LazyLock<Value> = LazyLock::new(config_json_schema);

pub(super) fn without_default(schema: &mut schemars::Schema) {
    schema.remove("default");
}

pub(super) fn pin_current_schema_version(schema: &mut schemars::Schema) {
    let Some(root) = schema.as_object_mut() else {
        return;
    };
    if let Some(version) = root
        .get_mut("properties")
        .and_then(|properties| properties.get_mut("schema_version"))
        .and_then(Value::as_object_mut)
    {
        version.remove("default");
        version.insert("const".into(), CURRENT_SCHEMA_VERSION.into());
    }
    if let Some(required) = root
        .entry("required")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
    {
        required.push("schema_version".into());
    }
}

pub(super) fn share_profile_fields_with_protocol_branches(schema: &mut schemars::Schema) {
    let strict_protocols = strict_protocols();
    let Some(profile) = schema.as_object_mut() else {
        return;
    };
    let shared_fields: Vec<String> = profile
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| properties.keys().cloned().collect())
        .unwrap_or_default();
    let branches = profile
        .get_mut("oneOf")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object_mut);
    for branch in branches {
        if let Some(properties) = branch.get_mut("properties").and_then(Value::as_object_mut) {
            for field in &shared_fields {
                properties.entry(field.clone()).or_insert(Value::Bool(true));
            }
        }
        if strict_protocols.contains(&branch["properties"]["protocol"]["const"]) {
            branch.insert("additionalProperties".into(), Value::Bool(false));
        }
    }
}

pub(super) fn check_branches_only_for_known_protocols(schema: &mut schemars::Schema) {
    let Some(profile) = schema.as_object_mut() else {
        return;
    };
    let Some(branches) = profile.remove("oneOf") else {
        return;
    };
    let protocols: Vec<Value> = branches
        .as_array()
        .into_iter()
        .flatten()
        .map(|branch| branch["properties"]["protocol"]["const"].clone())
        .collect();
    let known_protocol = serde_json::json!({ "enum": protocols });
    if let Some(properties) = profile
        .entry("properties")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
    {
        properties.insert("protocol".into(), known_protocol.clone());
    }
    profile.insert(
        "if".into(),
        serde_json::json!({ "properties": { "protocol": known_protocol } }),
    );
    profile.insert("then".into(), serde_json::json!({ "oneOf": branches }));
}

fn strict_protocols() -> Vec<Value> {
    let protocol_config = schemars::generate::SchemaSettings::draft07()
        .into_generator()
        .into_root_schema_for::<ProtocolConfig>()
        .to_value();
    protocol_config["oneOf"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|branch| branch.get("additionalProperties") == Some(&Value::Bool(false)))
        .map(|branch| branch["properties"]["protocol"]["const"].clone())
        .collect()
}

pub fn config_json_schema() -> Value {
    schemars::generate::SchemaSettings::draft07()
        .into_generator()
        .into_root_schema_for::<Config>()
        .to_value()
}

pub fn schema_diagnostics(document: &Value) -> Vec<ConfigDiagnostic> {
    let mut problems = Vec::new();
    Validator { root: &SCHEMA }.check(&SCHEMA, document, "", &mut problems);
    problems
        .into_iter()
        .map(|problem| {
            let location = if problem.pointer.is_empty() {
                "/"
            } else {
                &problem.pointer
            };
            let message = format!("{location}: {}", problem.message);
            ConfigDiagnostic::new(problem.pointer, message)
        })
        .collect()
}

struct Problem {
    pointer: String,
    message: String,
    mismatch: Option<Mismatch>,
}

enum Mismatch {
    Type(Vec<String>),
    Const(Value),
}

impl Problem {
    fn new(pointer: &str, message: impl Into<String>) -> Self {
        Self {
            pointer: pointer.to_string(),
            message: message.into(),
            mismatch: None,
        }
    }
}

struct Validator<'a> {
    root: &'a Value,
}

impl Validator<'_> {
    fn check(&self, schema: &Value, value: &Value, pointer: &str, out: &mut Vec<Problem>) {
        let Some(schema) = schema.as_object() else {
            return;
        };
        if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
            if let Some(target) = self.resolve(reference) {
                self.check(target, value, pointer, out);
            }
            return;
        }
        if let Some(types) = schema.get("type")
            && !matches_type(types, value)
        {
            out.push(type_mismatch(pointer, types));
            return;
        }
        if let Some(expected) = schema.get("const")
            && value != expected
        {
            out.push(Problem {
                mismatch: Some(Mismatch::Const(expected.clone())),
                ..Problem::new(pointer, format!("must be {expected}"))
            });
        }
        if let Some(allowed) = schema.get("enum").and_then(Value::as_array)
            && !allowed.contains(value)
        {
            out.push(Problem::new(
                pointer,
                format!("must be one of {}", list(allowed)),
            ));
        }
        check_number_bounds(schema, value, pointer, out);
        check_min_length(schema, value, pointer, out);
        check_format(schema, value, pointer, out);
        if let Some(object) = value.as_object() {
            self.check_object(schema, object, pointer, out);
        }
        if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
            for (index, item) in array.iter().enumerate() {
                self.check(items, item, &format!("{pointer}/{index}"), out);
            }
        }
        for branch in schema
            .get("allOf")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            self.check(branch, value, pointer, out);
        }
        for keyword in ["anyOf", "oneOf"] {
            if let Some(branches) = schema.get(keyword).and_then(Value::as_array) {
                self.check_alternatives(branches, value, pointer, out);
            }
        }
        if let (Some(condition), Some(consequence)) = (schema.get("if"), schema.get("then"))
            && self.accepts(condition, value)
        {
            self.check(consequence, value, pointer, out);
        }
    }

    fn accepts(&self, schema: &Value, value: &Value) -> bool {
        let mut problems = Vec::new();
        self.check(schema, value, "", &mut problems);
        problems.is_empty()
    }

    fn resolve(&self, reference: &str) -> Option<&Value> {
        let name = reference.strip_prefix("#/definitions/")?;
        self.root.get("definitions")?.get(name)
    }

    fn check_object(
        &self,
        schema: &Map<String, Value>,
        object: &Map<String, Value>,
        pointer: &str,
        out: &mut Vec<Problem>,
    ) {
        for required in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !object.contains_key(required) {
                out.push(Problem::new(
                    pointer,
                    format!("\"{required}\" is a required property"),
                ));
            }
        }
        let properties = schema.get("properties").and_then(Value::as_object);
        for (key, member) in object {
            let member_pointer = format!("{pointer}/{}", escape(key));
            match (
                properties.and_then(|properties| properties.get(key)),
                schema.get("additionalProperties"),
            ) {
                (Some(property), _) => self.check(property, member, &member_pointer, out),
                (None, Some(Value::Bool(false))) => out.push(Problem::new(
                    &member_pointer,
                    format!("unknown property \"{key}\""),
                )),
                (None, Some(additional)) => self.check(additional, member, &member_pointer, out),
                (None, None) => {}
            }
        }
    }

    fn check_alternatives(
        &self,
        branches: &[Value],
        value: &Value,
        pointer: &str,
        out: &mut Vec<Problem>,
    ) {
        let outcomes: Vec<Vec<Problem>> = branches
            .iter()
            .map(|branch| {
                let mut problems = Vec::new();
                self.check(branch, value, pointer, &mut problems);
                problems
            })
            .collect();
        if outcomes.iter().any(Vec::is_empty) {
            return;
        }
        let (shape_mismatches, relevant): (Vec<_>, Vec<_>) =
            outcomes.into_iter().partition(|problems| {
                problems
                    .iter()
                    .any(|problem| is_shape_mismatch(problem, pointer))
            });
        match <[Vec<Problem>; 1]>::try_from(relevant) {
            Ok([problems]) => out.extend(problems),
            Err(relevant) if !relevant.is_empty() => {
                out.push(Problem::new(pointer, "does not match any allowed shape"));
            }
            Err(_) => out.push(merged_mismatch(shape_mismatches, pointer)),
        }
    }
}

fn is_shape_mismatch(problem: &Problem, pointer: &str) -> bool {
    match problem.mismatch {
        Some(Mismatch::Const(_)) => true,
        Some(Mismatch::Type(_)) => problem.pointer == pointer,
        None => false,
    }
}

fn merged_mismatch(outcomes: Vec<Vec<Problem>>, pointer: &str) -> Problem {
    let mismatches: Vec<Problem> = outcomes
        .into_iter()
        .filter_map(|problems| {
            problems
                .into_iter()
                .find(|problem| is_shape_mismatch(problem, pointer))
        })
        .collect();
    let constants: Vec<Value> = mismatches
        .iter()
        .filter_map(|problem| match &problem.mismatch {
            Some(Mismatch::Const(expected)) => Some(expected.clone()),
            _ => None,
        })
        .collect();
    if let Some(first) = mismatches
        .iter()
        .find(|problem| matches!(problem.mismatch, Some(Mismatch::Const(_))))
    {
        return Problem::new(
            &first.pointer,
            format!("must be one of {}", list(&constants)),
        );
    }
    let types: Vec<String> = mismatches
        .into_iter()
        .filter_map(|problem| match problem.mismatch {
            Some(Mismatch::Type(types)) => Some(types),
            _ => None,
        })
        .flatten()
        .collect();
    type_mismatch_of(pointer, types)
}

fn matches_type(types: &Value, value: &Value) -> bool {
    type_names(types).iter().any(|name| match name.as_str() {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        _ => false,
    })
}

fn type_names(types: &Value) -> Vec<String> {
    match types {
        Value::String(name) => vec![name.clone()],
        Value::Array(names) => names
            .iter()
            .filter_map(Value::as_str)
            .map(String::from)
            .collect(),
        _ => Vec::new(),
    }
}

fn type_mismatch(pointer: &str, types: &Value) -> Problem {
    type_mismatch_of(pointer, type_names(types))
}

fn type_mismatch_of(pointer: &str, mut types: Vec<String>) -> Problem {
    types.dedup();
    let expected: Vec<_> = types.iter().map(|name| format!("\"{name}\"")).collect();
    Problem {
        mismatch: Some(Mismatch::Type(types)),
        ..Problem::new(
            pointer,
            format!("value is not of type {}", expected.join(" or ")),
        )
    }
}

fn check_number_bounds(
    schema: &Map<String, Value>,
    value: &Value,
    pointer: &str,
    out: &mut Vec<Problem>,
) {
    let Some(number) = value.as_f64() else {
        return;
    };
    if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64)
        && number < minimum
    {
        out.push(Problem::new(
            pointer,
            format!("value is less than the minimum of {minimum}"),
        ));
    }
    if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64)
        && number > maximum
    {
        out.push(Problem::new(
            pointer,
            format!("value is greater than the maximum of {maximum}"),
        ));
    }
}

fn check_min_length(
    schema: &Map<String, Value>,
    value: &Value,
    pointer: &str,
    out: &mut Vec<Problem>,
) {
    let (Some(minimum), Some(text)) = (
        schema.get("minLength").and_then(Value::as_u64),
        value.as_str(),
    ) else {
        return;
    };
    if (text.chars().count() as u64) < minimum {
        out.push(Problem::new(
            pointer,
            match minimum {
                1 => "must not be empty".to_string(),
                _ => format!("must be at least {minimum} characters long"),
            },
        ));
    }
}

fn check_format(schema: &Map<String, Value>, value: &Value, pointer: &str, out: &mut Vec<Problem>) {
    let (Some(format), Some(text)) = (schema.get("format").and_then(Value::as_str), value.as_str())
    else {
        return;
    };
    let valid = match format {
        "uuid" => uuid::Uuid::parse_str(text).is_ok(),
        "date" => chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").is_ok(),
        "date-time" => chrono::DateTime::parse_from_rfc3339(text).is_ok(),
        _ => true,
    };
    if !valid {
        out.push(Problem::new(
            pointer,
            format!("value is not a valid {format}"),
        ));
    }
}

fn list(values: &[Value]) -> String {
    let rendered: Vec<_> = values.iter().map(Value::to_string).collect();
    rendered.join(", ")
}

fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pointers(document: Value) -> Vec<(String, String)> {
        schema_diagnostics(&document)
            .into_iter()
            .map(|diagnostic| (diagnostic.pointer, diagnostic.message))
            .collect()
    }

    fn config_with_every_shape() -> Value {
        let tls = json!({
            "server_name": "sni.example", "insecure": true, "alpn": ["h2"],
            "utls_fingerprint": "chrome", "ech": { "enabled": true, "config": ["AEX+"] }
        });
        let with_tls = |profile: Value| {
            let mut profile = profile;
            profile
                .as_object_mut()
                .unwrap()
                .extend(tls.as_object().unwrap().clone());
            profile
        };
        let base = |protocol: &str| {
            json!({
                "name": protocol, "address": "vpn.example",
                "port": 443, "protocol": protocol, "tags": ["t"]
            })
        };
        let merge = |mut left: Value, right: Value| {
            left.as_object_mut()
                .unwrap()
                .extend(right.as_object().unwrap().clone());
            left
        };
        let uuid = crate::test_helpers::TEST_UUID;
        json!({
            "$schema": "file:///run/user/1000/kvn/profiles.schema.json",
            "schema_version": super::super::CURRENT_SCHEMA_VERSION,
            "profiles": [
                merge(base("vless"), json!({
                    "uuid": uuid, "flow": "xtls-rprx-vision", "security": "reality",
                    "transport": { "type": "grpc", "service_name": "svc" },
                    "server_name": "sni.example", "utls_fingerprint": "chrome",
                    "reality": { "public_key": "pk", "short_id": "ab", "server_name": "sni", "spider_x": "/" }
                })),
                with_tls(merge(base("vmess"), json!({
                    "uuid": uuid, "alter_id": 0, "security": "auto", "global_padding": true,
                    "transport": { "type": "ws", "path": "/ws", "host": "h", "headers": { "X": "y" } }
                }))),
                with_tls(merge(base("trojan"), json!({
                    "password": "p",
                    "transport": { "type": "xhttp", "path": "/x" },
                    "share_link_params": { "mode": "auto", "mtu": 1350, "extra": { "headers": { "X": "y" } } }
                }))),
                merge(base("shadowsocks"), json!({ "method": "2022-blake3-aes-256-gcm", "password": "p" })),
                with_tls(merge(base("hysteria2"), json!({
                    "password": "p", "up_mbps": 10, "down_mbps": 50,
                    "obfs": { "type": "salamander", "password": "o" }
                }))),
                with_tls(merge(base("tuic"), json!({
                    "uuid": uuid, "password": "p", "congestion_control": "bbr",
                    "udp_relay_mode": "quic", "zero_rtt_handshake": true
                }))),
                with_tls(merge(base("shadowtls"), json!({
                    "version": 3, "password": "p", "method": "aes-256-gcm", "ss_password": "s"
                }))),
                with_tls(merge(base("anytls"), json!({
                    "password": "p", "idle_session_check_interval": "30s", "idle_session_timeout": "30s"
                }))),
                with_tls(merge(base("naive"), json!({ "username": "u", "password": "p", "quic": true }))),
                merge(base("socks"), json!({ "version": "5", "username": "u", "password": "p" })),
                with_tls(merge(base("http"), json!({ "username": "u", "password": "p" }))),
                merge(base("ssh"), json!({
                    "user": "u", "password": "p", "private_key_path": "/k",
                    "host_key": ["ssh-ed25519 AAAA"], "host_key_algorithms": ["ssh-ed25519"]
                }))
            ],
            "subscriptions": [{
                "id": "22222222-2222-2222-2222-222222222222", "name": "S",
                "url": "https://sub.example/s", "auto_update": "every_1d",
                "last_updated": "2026-01-01T00:00:00+03:00", "next_auto_update": "2026-01-02",
                "retry_state": { "consecutive_failures": 1, "retry_at": "2026-01-01T01:00:00+03:00", "attempt_date": "2026-01-01" },
                "send_hwid": true, "hwid": "device"
            }],
            "settings": {
                "last_connected_profile": uuid,
                "tun_interface": "kvn0", "auto_connect": true, "kill_switch": true,
                "theme": "nord", "icons": "unicode", "hwid": "h",
                "allow_insecure_http_subscriptions": true,
                "logs": { "level": "debug", "line_retention": { "app": 2000, "singbox": 2000 } },
                "connectivity_probe": { "enabled": true, "url": "https://probe.example/204" },
                "geo_routing": {
                    "current_region": "ru", "auto_update": "every_7d",
                    "selected_region_modes": { "ru": "bypass_ru", "cn": "only_cn" },
                    "service_routes": { "steam": "direct", "telegram": "proxy" }
                },
                "dns": {
                    "current_preset": "home",
                    "custom_presets": [{
                        "name": "home",
                        "servers": [
                            { "type": "local", "tag": "local" },
                            { "type": "udp", "tag": "udp", "server": "1.1.1.1", "server_port": 53 },
                            { "type": "tcp", "tag": "tcp", "server": "1.1.1.1" },
                            { "type": "tls", "tag": "tls", "server": "8.8.8.8" },
                            { "type": "https", "tag": "https", "server": "1.1.1.1", "path": "/dns-query" },
                            { "type": "quic", "tag": "quic", "server": "9.9.9.9" }
                        ],
                        "rules": [{ "server": "local", "domain_suffix": ["lan"], "disable_cache": true }],
                        "final_server": "https"
                    }],
                    "strategy": "ipv4_only", "fakeip_enabled": true,
                    "fakeip_ranges": { "inet4_range": "198.19.0.0/16", "inet6_range": "fc00::/18" }
                }
            }
        })
    }

    #[test]
    fn every_config_shape_serde_accepts_matches_the_schema() {
        let document = config_with_every_shape();
        let config: Config = serde_json::from_value(document.clone()).unwrap();
        assert_eq!(config.diagnostics(), []);
        assert_eq!(pointers(document), []);
        assert_eq!(pointers(serde_json::to_value(&config).unwrap()), []);
        assert_eq!(
            pointers(serde_json::to_value(Config::for_first_run()).unwrap()),
            []
        );
    }

    #[test]
    fn profile_checks_the_protocol_branches_only_for_a_known_protocol() {
        let profile = &config_json_schema()["definitions"]["Profile"];
        let branch_protocols: Vec<&Value> = profile["then"]["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .map(|branch| &branch["properties"]["protocol"]["const"])
            .collect();
        let listed: Vec<&Value> = profile["properties"]["protocol"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .collect();
        assert_eq!(listed.len(), 12);
        assert_eq!(listed, branch_protocols);
        assert_eq!(
            profile["if"]["properties"]["protocol"],
            profile["properties"]["protocol"]
        );
    }

    #[test]
    fn every_protocol_branch_lists_the_shared_profile_fields() {
        let profile = &config_json_schema()["definitions"]["Profile"];
        for branch in profile["then"]["oneOf"].as_array().unwrap() {
            for field in ["id", "name", "address", "port", "tags", "subscription_id"] {
                assert!(
                    branch["properties"].get(field).is_some(),
                    "{} lacks {field}",
                    branch["properties"]["protocol"]["const"]
                );
            }
        }
    }

    #[test]
    fn reports_every_structural_problem_with_its_pointer() {
        let document = json!({
            "schema_version": CURRENT_SCHEMA_VERSION,
            "profiles": [
                { "protocol": "vless", "address": "a", "port": "443", "uuid": "u" },
                { "protocol": "trojan", "name": "t", "address": "b", "port": 1 },
                { "protocol": "vlesss", "name": "x", "address": "c", "port": 1 }
            ],
            "settings": { "unknown": true }
        });
        let found = pointers(document);
        let found_pointers: Vec<_> = found.iter().map(|(pointer, _)| pointer.as_str()).collect();
        assert_eq!(
            found_pointers,
            [
                "/profiles/0",
                "/profiles/0/port",
                "/profiles/1",
                "/profiles/2/protocol",
                "/settings/unknown",
            ]
        );
        assert_eq!(found[0].1, "/profiles/0: \"name\" is a required property");
        assert!(found[2].1.contains("\"password\" is a required property"));
        assert!(
            found[3]
                .1
                .starts_with("/profiles/2/protocol: must be one of \"vless\", \"vmess\"")
        );
        assert_eq!(
            found[4].1,
            "/settings/unknown: unknown property \"unknown\""
        );
    }

    #[test]
    fn reports_values_rejected_inside_optional_and_bounded_fields() {
        let document = json!({
            "schema_version": CURRENT_SCHEMA_VERSION,
            "profiles": [{
                "id": "not-a-uuid", "protocol": "vless", "name": "n", "address": "a",
                "port": 70000, "uuid": "u", "reality": { "public_key": "k" }, "ech": 5
            }],
            "subscriptions": [{ "name": "s", "url": "u", "last_updated": "yesterday" }],
            "settings": {
                "dns": { "custom_presets": [{ "name": "p", "servers": [{ "type": "udp", "tag": "t" }], "final_server": "t" }] },
                "icons": "emoji"
            }
        });
        assert_eq!(
            pointers(document)
                .into_iter()
                .map(|(_, message)| message)
                .collect::<Vec<_>>(),
            [
                "/profiles/0/id: value is not a valid uuid",
                "/profiles/0/port: value is greater than the maximum of 65535",
                "/profiles/0/ech: value is not of type \"object\" or \"null\"",
                "/profiles/0/reality: \"short_id\" is a required property",
                "/profiles/0/reality: \"server_name\" is a required property",
                "/profiles/0/reality: \"spider_x\" is a required property",
                "/settings/dns/custom_presets/0/servers/0: \"server\" is a required property",
                "/settings/icons: must be one of \"nerd\", \"unicode\"",
                "/subscriptions/0/last_updated: value is not a valid date-time",
            ]
        );
    }

    #[test]
    fn strict_protocols_and_dns_servers_reject_every_unknown_field() {
        let profile = |protocol: &str, fields: Value| {
            let mut profile = json!({
                "id": crate::test_helpers::TEST_UUID, "name": "n", "address": "a.example",
                "port": 1, "tags": ["t"], "protocol": protocol,
                "typo_one": 1, "typo_two": 2
            });
            profile
                .as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            profile
        };
        let document = json!({
            "schema_version": CURRENT_SCHEMA_VERSION,
            "profiles": [
                profile("socks", json!({ "version": "5" })),
                profile("ssh", json!({ "user": "u" })),
                profile("shadowsocks", json!({ "method": "aes-256-gcm", "password": "p" })),
                profile("vless", json!({ "uuid": crate::test_helpers::TEST_UUID }))
            ],
            "settings": {
                "dns": { "custom_presets": [{ "name": "p", "servers": [{ "type": "local", "tag": "l", "typo": 1 }], "final_server": "l" }] }
            }
        });
        let found: Vec<_> = pointers(document)
            .into_iter()
            .map(|(pointer, _)| pointer)
            .collect();
        assert_eq!(
            found,
            [
                "/profiles/0/typo_one",
                "/profiles/0/typo_two",
                "/profiles/1/typo_one",
                "/profiles/1/typo_two",
                "/profiles/2/typo_one",
                "/profiles/2/typo_two",
                "/settings/dns/custom_presets/0/servers/0/typo",
            ]
        );
        let lenient: Result<Config, _> = serde_json::from_value(json!({
            "profiles": [profile("vless", json!({ "uuid": crate::test_helpers::TEST_UUID }))]
        }));
        assert!(
            lenient.is_ok(),
            "serde ignores unknown fields of lenient protocols"
        );
    }

    #[test]
    fn value_constraints_match_the_semantic_checks() {
        let document = json!({
            "schema_version": CURRENT_SCHEMA_VERSION,
            "profiles": [
                { "protocol": "trojan", "name": "", "address": "", "port": 0, "password": "" },
                {
                    "protocol": "shadowtls", "name": "s", "address": "a", "port": 1,
                    "version": 3, "password": "p", "ss_password": ""
                }
            ],
            "settings": {
                "theme": "missing",
                "logs": { "level": "verbose", "line_retention": { "app": 999 } },
                "dns": {
                    "custom_presets": [
                        { "name": "", "servers": [{ "type": "local", "tag": "l" }], "final_server": "l" }
                    ],
                    "strategy": "ipv6_only"
                }
            }
        });
        let mut messages: Vec<_> = pointers(document)
            .into_iter()
            .map(|(_, message)| message)
            .collect();
        let theme = messages.pop().unwrap();
        assert!(theme.starts_with("/settings/theme: must be one of \"omarchy\", "));
        assert_eq!(
            messages,
            [
                "/profiles/0/address: must not be empty",
                "/profiles/0/name: must not be empty",
                "/profiles/0/port: value is less than the minimum of 1",
                "/profiles/0/password: must not be empty",
                "/profiles/1/ss_password: must not be empty",
                "/settings/dns/custom_presets/0/name: must not be empty",
                "/settings/dns/strategy: must be one of \"prefer_ipv4\", \"ipv4_only\"",
                "/settings/logs/level: must be one of \"trace\", \"debug\", \"info\", \"warn\", \"error\"",
                "/settings/logs/line_retention/app: value is less than the minimum of 1000",
            ]
        );
        let themes = std::iter::once(super::super::OMARCHY_THEME_SENTINEL)
            .chain(super::super::settings::BUNDLED_THEME_NAMES.iter().copied());
        let levels = super::super::settings::LOG_LEVELS.iter().copied();
        for (theme, level) in themes.zip(levels.cycle()) {
            let settings = json!({
                "theme": theme,
                "logs": { "level": level, "line_retention": { "app": 1_000, "singbox": 1_000 } }
            });
            let document =
                json!({ "schema_version": CURRENT_SCHEMA_VERSION, "settings": settings });
            assert_eq!(pointers(document), [], "{theme} {level}");
            let parsed: super::super::Settings = serde_json::from_value(settings).unwrap();
            assert_eq!(parsed.diagnostics(), []);
        }
    }

    #[test]
    fn legacy_dns_fields_are_not_part_of_the_current_schema() {
        let document = json!({
            "schema_version": CURRENT_SCHEMA_VERSION,
            "settings": { "dns": { "servers": [], "rules": [], "final_server": "remote" } }
        });
        let found: Vec<_> = pointers(document)
            .into_iter()
            .map(|(pointer, _)| pointer)
            .collect();
        assert_eq!(
            found,
            [
                "/settings/dns/final_server",
                "/settings/dns/rules",
                "/settings/dns/servers",
            ]
        );
    }

    #[test]
    fn schema_version_must_stay_the_current_one() {
        assert_eq!(
            pointers(json!({})),
            [(
                "".into(),
                "/: \"schema_version\" is a required property".into()
            )]
        );
        assert_eq!(
            pointers(json!({ "schema_version": 0 })),
            [(
                "/schema_version".into(),
                format!("/schema_version: must be {CURRENT_SCHEMA_VERSION}")
            )]
        );
    }

    #[test]
    fn schema_uses_only_keywords_the_validator_understands() {
        const UNDERSTOOD: &[&str] = &[
            "$ref",
            "additionalProperties",
            "allOf",
            "anyOf",
            "const",
            "enum",
            "format",
            "if",
            "items",
            "maximum",
            "minLength",
            "minimum",
            "oneOf",
            "properties",
            "required",
            "then",
            "type",
        ];
        const ANNOTATIONS: &[&str] = &[
            "$schema",
            "default",
            "definitions",
            "description",
            "title",
            "writeOnly",
        ];
        fn walk(schema: &Value, found: &mut std::collections::BTreeSet<String>) {
            let Some(object) = schema.as_object() else {
                return;
            };
            for (keyword, value) in object {
                found.insert(keyword.clone());
                match keyword.as_str() {
                    "properties" | "definitions" => {
                        value
                            .as_object()
                            .into_iter()
                            .flatten()
                            .for_each(|(_, s)| walk(s, found));
                    }
                    "allOf" | "anyOf" | "oneOf" => {
                        value
                            .as_array()
                            .into_iter()
                            .flatten()
                            .for_each(|s| walk(s, found));
                    }
                    "items" | "additionalProperties" | "if" | "then" => walk(value, found),
                    _ => {}
                }
            }
        }
        let mut found = std::collections::BTreeSet::new();
        walk(&config_json_schema(), &mut found);
        let unknown: Vec<_> = found
            .iter()
            .filter(|keyword| {
                !UNDERSTOOD.contains(&keyword.as_str()) && !ANNOTATIONS.contains(&keyword.as_str())
            })
            .collect();
        assert!(
            unknown.is_empty(),
            "validator does not implement {unknown:?}"
        );
    }

    #[test]
    fn generated_ids_carry_no_fixed_default() {
        let schema = config_json_schema();
        for definition in ["Profile", "Subscription"] {
            assert_eq!(
                schema["definitions"][definition]["properties"]["id"].get("default"),
                None
            );
        }
    }
}
