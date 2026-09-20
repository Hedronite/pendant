use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RpcRequest {
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RpcResponse {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RpcError {
    pub code: String,
    pub message: String,
}

impl RpcRequest {
    pub fn new(id: impl Into<String>, method: impl Into<String>, params: Value) -> Self {
        Self {
            id: id.into(),
            method: method.into(),
            params,
        }
    }

    pub fn to_line(&self) -> Result<String, serde_json::Error> {
        let mut line = serde_json::to_string(self)?;
        line.push('\n');
        Ok(line)
    }
}

/// If the CLI printed a full RPC envelope, unwrap `.result`; otherwise return as-is.
pub fn unwrap_result(value: Value) -> Value {
    match value {
        Value::Object(map) if map.contains_key("result") || map.contains_key("error") => {
            if let Some(result) = map.get("result") {
                return result.clone();
            }
            Value::Object(map)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_line_is_ndjson() {
        let req = RpcRequest::new("p-1", "session.snapshot", json!({}));
        let line = req.to_line().unwrap();
        assert!(line.ends_with('\n'));
        let parsed: RpcRequest = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(parsed.method, "session.snapshot");
        assert_eq!(parsed.id, "p-1");
    }

    #[test]
    fn unwrap_result_strips_envelope() {
        let raw = json!({"id":"x","result":{"type":"pong"}});
        assert_eq!(unwrap_result(raw), json!({"type":"pong"}));
    }
}
