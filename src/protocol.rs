use std::fmt;

use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct IncomingMessage {
    #[serde(rename = "type")]
    kind: String,
    #[serde(flatten)]
    payload: Map<String, Value>,
}

impl IncomingMessage {
    pub fn is_start_game(&self) -> bool {
        self.kind == "start_game"
    }

    pub fn is_end_game(&self) -> bool {
        self.kind == "end_game"
    }

    pub fn is_validation_result(&self) -> bool {
        self.kind == "validation_result"
    }

    pub fn request_action(&self) -> Result<Option<RequestAction<'_>>, ProtocolError> {
        if self.kind != "request_action" {
            return Ok(None);
        }

        let possible_actions = self.payload.get("possible_actions").ok_or(ProtocolError::MissingField("possible_actions"))?.as_array().ok_or(ProtocolError::InvalidFieldType("possible_actions"))?;
        let observation = self.payload.get("observation").ok_or(ProtocolError::MissingField("observation"))?.as_str().ok_or(ProtocolError::InvalidFieldType("observation"))?;

        Ok(Some(RequestAction { possible_actions, observation }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RequestAction<'a> {
    pub possible_actions: &'a [Value],
    pub observation: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolError {
    MissingField(&'static str),
    InvalidFieldType(&'static str),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingField(field) => write!(f, "missing field: {field}"),
            Self::InvalidFieldType(field) => write!(f, "invalid field type: {field}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_request_action() {
        let message: IncomingMessage = serde_json::from_str(
            r#"{
                "type": "request_action",
                "possible_actions": [
                    {"type":"none"},
                    {"type":"dahai","actor":0,"pai":"3m","tsumogiri":true}
                ],
                "observation": "ZmFrZQ=="
            }"#,
        )
        .unwrap();

        let request = message.request_action().unwrap().unwrap();
        assert_eq!(request.observation, "ZmFrZQ==");
        assert_eq!(request.possible_actions.len(), 2);
    }

    #[test]
    fn identifies_boundary_events() {
        let start: IncomingMessage = serde_json::from_str(r#"{"type":"start_game","id":0}"#).unwrap();
        let end: IncomingMessage = serde_json::from_str(r#"{"type":"end_game","scores":[25000,25000,25000,25000]}"#).unwrap();

        assert!(start.is_start_game());
        assert!(end.is_end_game());
    }
}
