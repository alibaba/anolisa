use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::auth::BearerAuth;

/// One daemon request. Unknown envelope fields are rejected.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DaemonRequest {
    /// Fixed allowlisted method.
    pub method: String,
    /// Method-specific object.
    #[serde(default = "empty_object")]
    pub params: Value,
    /// Optional management authentication material interpreted by the handler.
    #[serde(default)]
    pub auth: Option<BearerAuth>,
    /// W3C trace parent carrier.
    #[serde(default)]
    pub traceparent: Option<String>,
    /// W3C trace state carrier.
    #[serde(default)]
    pub tracestate: Option<String>,
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}
