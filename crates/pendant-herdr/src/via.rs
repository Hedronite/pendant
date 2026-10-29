use serde_json::{json, Value};

/// Which transport produced a herdr reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    Socket,
    Cli,
}

impl Via {
    pub fn as_str(self) -> &'static str {
        match self {
            Via::Socket => "socket",
            Via::Cli => "cli",
        }
    }
}

/// Put `via` on an object reply. A non-object is wrapped so the field is still present.
pub(crate) fn stamp(value: Value, via: Via) -> Value {
    match value {
        Value::Object(mut map) => {
            map.insert("via".into(), Value::String(via.as_str().into()));
            Value::Object(map)
        }
        other => json!({ "via": via.as_str(), "value": other }),
    }
}
