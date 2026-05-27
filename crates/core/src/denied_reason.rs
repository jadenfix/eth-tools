//! Typed envelope for every "this is not allowed" path through the API.
//!
//! Per plan §9.1: errors are not opaque — they carry a code, the evaluator
//! that produced them, expected/got pairs when meaningful, and a human hint.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeniedReason {
    pub code: String,
    pub policy_version: String,
    pub evaluator: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub got: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub override_hint: Option<String>,
}

impl DeniedReason {
    pub fn new(code: impl Into<String>, evaluator: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            policy_version: "v1".into(),
            evaluator: evaluator.into(),
            expected: None,
            got: None,
            override_hint: None,
        }
    }

    pub fn with_got(mut self, got: serde_json::Value) -> Self {
        self.got = Some(got);
        self
    }

    pub fn with_expected(mut self, expected: serde_json::Value) -> Self {
        self.expected = Some(expected);
        self
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.override_hint = Some(hint.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trip_serde() {
        let dr = DeniedReason::new("RECIPIENT_NOT_ALLOWED", "wallet.allowlist")
            .with_got(json!({"to": "0xdead"}))
            .with_hint("must be one of the three ERC-8004 registries");
        let s = serde_json::to_string(&dr).unwrap();
        let parsed: DeniedReason = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, dr);
    }

    #[test]
    fn omits_none_fields() {
        let dr = DeniedReason::new("RATE_LIMITED", "api.rate_limit");
        let s = serde_json::to_string(&dr).unwrap();
        assert!(!s.contains("expected"));
        assert!(!s.contains("got"));
        assert!(!s.contains("override_hint"));
    }
}
