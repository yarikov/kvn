use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use super::Settings;

pub const SETTINGS_SCHEMA_VERSION: u32 = 1;

const JSON_SCHEMA_KEY: &str = "$schema";
const SCHEMA_VERSION_KEY: &str = "schema_version";
const PROFILES_ONLY_KEY: &str = "last_connected_profile";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsDocument {
    pub json_schema: Option<String>,
    pub schema_version: u32,
    pub settings: Settings,
}

impl SettingsDocument {
    pub fn new(mut settings: Settings) -> Self {
        settings.last_connected_profile = None;
        Self {
            json_schema: None,
            schema_version: SETTINGS_SCHEMA_VERSION,
            settings,
        }
    }
}

impl Default for SettingsDocument {
    fn default() -> Self {
        Self::new(Settings::default())
    }
}

#[derive(Serialize)]
struct SettingsDocumentFields<'a> {
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    json_schema: &'a Option<String>,
    schema_version: u32,
    #[serde(flatten)]
    settings: &'a Settings,
}

impl Serialize for SettingsDocument {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let settings = Settings {
            last_connected_profile: None,
            ..self.settings.clone()
        };
        SettingsDocumentFields {
            json_schema: &self.json_schema,
            schema_version: self.schema_version,
            settings: &settings,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SettingsDocument {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = Map::deserialize(deserializer)?;
        let json_schema = fields
            .remove(JSON_SCHEMA_KEY)
            .map(serde_json::from_value)
            .transpose()
            .map_err(D::Error::custom)?;
        let schema_version = fields
            .remove(SCHEMA_VERSION_KEY)
            .map(serde_json::from_value)
            .transpose()
            .map_err(D::Error::custom)?
            .ok_or_else(|| D::Error::missing_field(SCHEMA_VERSION_KEY))?;
        if fields.contains_key(PROFILES_ONLY_KEY) {
            return Err(D::Error::custom(format!(
                "unknown field `{PROFILES_ONLY_KEY}`: it belongs to profiles.json"
            )));
        }
        let settings = Settings::deserialize(Value::Object(fields)).map_err(D::Error::custom)?;
        Ok(Self {
            json_schema,
            schema_version,
            settings,
        })
    }
}

impl JsonSchema for SettingsDocument {
    fn schema_name() -> Cow<'static, str> {
        "SettingsDocument".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let mut schema = Settings::json_schema(generator);
        schema.insert(
            "description".into(),
            "Application settings: theme, DNS, kill switch, routing mode, update schedules.".into(),
        );
        if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            properties.remove(PROFILES_ONLY_KEY);
            properties.insert(
                JSON_SCHEMA_KEY.into(),
                serde_json::json!({ "type": "string" }),
            );
            properties.insert(
                SCHEMA_VERSION_KEY.into(),
                serde_json::json!({
                    "description": "Schema version of the persisted file.",
                    "type": "integer",
                    "const": SETTINGS_SCHEMA_VERSION,
                }),
            );
        }
        if let Some(required) = schema
            .ensure_object()
            .entry("required")
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
        {
            required.push(SCHEMA_VERSION_KEY.into());
        }
        schema
    }
}
