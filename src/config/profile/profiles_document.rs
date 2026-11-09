use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{CURRENT_SCHEMA_VERSION, Profile, Subscription};

/// Root structure of `profiles.json`: profiles, subscriptions and the last
/// connected profile. Settings live in `settings.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = super::json_schema::pin_current_schema_version)]
pub struct ProfilesDocument {
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub json_schema: Option<String>,
    /// Schema version of the persisted file. See [`CURRENT_SCHEMA_VERSION`].
    pub schema_version: u32,
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subscriptions: Vec<Subscription>,
    /// The profile auto-connect starts with: the one connected most recently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_connected_profile: Option<Uuid>,
}

impl Default for ProfilesDocument {
    fn default() -> Self {
        Self {
            json_schema: None,
            schema_version: CURRENT_SCHEMA_VERSION,
            profiles: Vec::new(),
            subscriptions: Vec::new(),
            last_connected_profile: None,
        }
    }
}
