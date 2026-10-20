//! Shared OpenCode tabs/focus token codec.
//!
//! OpenCode's Herdr integration publishes an internal pane metadata token
//! [`TABS_TOKEN`] describing the root OpenCode conversation tabs hosted by one
//! pane. The sidebar client parses that token with [`parse_snapshot`] to render
//! one row per tab. A second short-lived token [`FOCUS_TOKEN`] lets the client
//! ask the owning pane to select a tab; the backend validates it against the
//! live, unexpired snapshot before storing it.
//!
//! Both tokens travel through the existing `pane.report_metadata` method and
//! the existing `PaneInfo.tokens` readback, so no new method or wire field is
//! required. The backend accepts the reserved token values before the generic
//! 80-character token truncation, so ordinary token behavior is unchanged.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Reserved pane metadata token carrying the OpenCode root tab snapshot.
pub const TABS_TOKEN: &str = "herdr_oc_tabs";
/// Source identity required for a [`TABS_TOKEN`] report.
pub const TABS_SOURCE: &str = "herdr:opencode:tabs";
/// Reserved pane metadata token carrying a one-shot tab focus request.
pub const FOCUS_TOKEN: &str = "herdr_oc_focus";
/// Source identity required for a [`FOCUS_TOKEN`] report.
pub const FOCUS_SOURCE: &str = "herdr:opencode:focus";
/// Canonical agent label attached to reserved OpenCode reports.
pub const OPENCODE_AGENT: &str = "opencode";
/// Fixed lifetime for a stored tab snapshot.
pub const TABS_TTL_MS: u64 = 15_000;
/// Fixed lifetime for a stored focus request.
pub const FOCUS_TTL_MS: u64 = 3_000;
/// Maximum number of distinct tabs in one snapshot.
pub const MAX_TABS: usize = 64;
/// Maximum length of a `sessionID` in Unicode scalar values.
pub const MAX_SESSION_ID_CHARS: usize = 128;
/// Maximum length of a tab title in Unicode scalar values.
pub const MAX_TITLE_CHARS: usize = 128;
/// Maximum size of the raw snapshot JSON in UTF-8 bytes.
pub const MAX_SNAPSHOT_BYTES: usize = 65_536;
/// Exact length of an `owner` or `requestID` hex string.
pub const HEX_ID_CHARS: usize = 32;

const SCHEMA_VERSION: u32 = 1;

/// One OpenCode root conversation tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tab {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    pub title: String,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub busy: bool,
    #[serde(default)]
    pub attention: bool,
    #[serde(default)]
    pub unread: bool,
}

/// Validated OpenCode root tab snapshot for one owning pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TabSnapshot {
    pub v: u32,
    pub owner: String,
    pub tabs: Vec<Tab>,
    /// Server wall-clock expiry, added by the backend before storage.
    #[serde(
        rename = "expiresAtMs",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub expires_at_ms: Option<u64>,
}

/// Validated request asking one OpenCode pane to select a root tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FocusRequest {
    pub v: u32,
    pub owner: String,
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "requestID")]
    pub request_id: String,
    /// Server wall-clock expiry, added by the backend before storage.
    #[serde(
        rename = "expiresAtMs",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub expires_at_ms: Option<u64>,
}

/// Validation failure with a stable API error code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabError {
    pub code: &'static str,
    pub message: String,
}

impl TabError {
    fn snapshot(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_opencode_tabs",
            message: message.into(),
        }
    }

    fn focus(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_opencode_focus",
            message: message.into(),
        }
    }
}

impl std::fmt::Display for TabError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for TabError {}

/// Parses and validates a [`TABS_TOKEN`] value.
pub fn parse_snapshot(value: &str) -> Result<TabSnapshot, TabError> {
    if value.len() > MAX_SNAPSHOT_BYTES {
        return Err(TabError::snapshot(format!(
            "opencode tabs snapshot must be {MAX_SNAPSHOT_BYTES} bytes or fewer"
        )));
    }
    let snapshot: TabSnapshot = serde_json::from_str(value)
        .map_err(|err| TabError::snapshot(format!("invalid opencode tabs snapshot: {err}")))?;
    validate_snapshot(&snapshot)?;
    Ok(snapshot)
}

/// Validates an already-decoded snapshot.
pub fn validate_snapshot(snapshot: &TabSnapshot) -> Result<(), TabError> {
    if snapshot.v != SCHEMA_VERSION {
        return Err(TabError::snapshot("unsupported opencode tabs version"));
    }
    validate_hex_id(&snapshot.owner)
        .map_err(|message| TabError::snapshot(format!("invalid opencode tabs owner: {message}")))?;
    if snapshot.tabs.len() > MAX_TABS {
        return Err(TabError::snapshot(format!(
            "opencode tabs snapshot may contain at most {MAX_TABS} tabs"
        )));
    }
    let mut session_ids = std::collections::HashSet::new();
    let mut active = 0usize;
    for tab in &snapshot.tabs {
        validate_session_id(&tab.session_id)
            .map_err(|message| TabError::snapshot(format!("invalid tab sessionID: {message}")))?;
        validate_title(&tab.title)
            .map_err(|message| TabError::snapshot(format!("invalid tab title: {message}")))?;
        if !session_ids.insert(tab.session_id.as_str()) {
            return Err(TabError::snapshot(
                "opencode tabs sessionIDs must be unique",
            ));
        }
        if tab.active {
            active += 1;
        }
    }
    if active > 1 {
        return Err(TabError::snapshot(
            "opencode tabs snapshot may mark at most one tab active",
        ));
    }
    Ok(())
}

/// Parses and validates a [`FOCUS_TOKEN`] value.
pub fn parse_focus_request(value: &str) -> Result<FocusRequest, TabError> {
    let request: FocusRequest = serde_json::from_str(value)
        .map_err(|err| TabError::focus(format!("invalid opencode focus request: {err}")))?;
    validate_focus_request(&request)?;
    Ok(request)
}

/// Validates an already-decoded focus request.
pub fn validate_focus_request(request: &FocusRequest) -> Result<(), TabError> {
    if request.v != SCHEMA_VERSION {
        return Err(TabError::focus("unsupported opencode focus version"));
    }
    validate_hex_id(&request.owner)
        .map_err(|message| TabError::focus(format!("invalid opencode focus owner: {message}")))?;
    validate_hex_id(&request.request_id).map_err(|message| {
        TabError::focus(format!("invalid opencode focus requestID: {message}"))
    })?;
    validate_session_id(&request.session_id).map_err(|message| {
        TabError::focus(format!("invalid opencode focus sessionID: {message}"))
    })?;
    Ok(())
}

/// Serializes a snapshot for storage (adds the `expiresAtMs` field).
pub fn encode_snapshot(snapshot: &TabSnapshot) -> String {
    serde_json::to_string(snapshot).unwrap_or_default()
}

/// Serializes a focus request for storage (adds the `expiresAtMs` field).
pub fn encode_focus_request(request: &FocusRequest) -> String {
    serde_json::to_string(request).unwrap_or_default()
}

/// Returns the reserved token key in `tokens`, if any.
pub fn reserved_token_key(tokens: &HashMap<String, Option<String>>) -> Option<&str> {
    let mut reserved = tokens
        .keys()
        .filter(|key| key.as_str() == TABS_TOKEN || key.as_str() == FOCUS_TOKEN);
    let first = reserved.next()?;
    Some(first.as_str())
}

/// Required report source for a reserved token key.
pub fn source_for_token(token: &str) -> Option<&'static str> {
    match token {
        TABS_TOKEN => Some(TABS_SOURCE),
        FOCUS_TOKEN => Some(FOCUS_SOURCE),
        _ => None,
    }
}

/// Current wall-clock time in Unix milliseconds.
pub fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// True when the stored snapshot `expires_at_ms` is still in the future.
pub fn snapshot_is_live(snapshot: &TabSnapshot, now_ms: u64) -> bool {
    snapshot
        .expires_at_ms
        .is_none_or(|expires| expires > now_ms)
}

fn validate_hex_id(value: &str) -> Result<(), String> {
    if value.len() != HEX_ID_CHARS
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("expected {HEX_ID_CHARS} lowercase hex characters"));
    }
    Ok(())
}

fn validate_session_id(value: &str) -> Result<(), String> {
    if value.is_empty() || value.chars().count() > MAX_SESSION_ID_CHARS {
        return Err(format!("expected 1 to {MAX_SESSION_ID_CHARS} characters"));
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
    {
        return Err("expected ASCII letters, digits, underscore, or hyphen".into());
    }
    Ok(())
}

fn validate_title(value: &str) -> Result<(), String> {
    if value.chars().count() > MAX_TITLE_CHARS {
        return Err(format!(
            "expected {MAX_TITLE_CHARS} Unicode scalar values or fewer"
        ));
    }
    if value.chars().any(char::is_control) {
        return Err("expected no control characters".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_json(owner: &str, tabs: &str) -> String {
        format!(r#"{{"v":1,"owner":"{owner}","tabs":[{tabs}]}}"#)
    }

    fn owner() -> &'static str {
        "0123456789abcdef0123456789abcdef"
    }

    #[test]
    fn opencode_tabs_parses_valid_snapshot_and_renames_fields() {
        let value = snapshot_json(
            owner(),
            r#"{"sessionID":"ses-1","title":"Root","active":true,"busy":true,"attention":false,"unread":true}"#,
        );
        let parsed = parse_snapshot(&value).unwrap();
        assert_eq!(parsed.v, 1);
        assert_eq!(parsed.owner, owner());
        assert_eq!(parsed.tabs.len(), 1);
        assert_eq!(parsed.tabs[0].session_id, "ses-1");
        assert!(parsed.tabs[0].unread);
        assert_eq!(parsed.expires_at_ms, None);
    }

    #[test]
    fn opencode_tabs_rejects_unknown_fields() {
        let value = format!(r#"{{"v":1,"owner":"{}","tabs":[],"extra":true}}"#, owner());
        assert!(parse_snapshot(&value).is_err());
        let value = snapshot_json(
            owner(),
            r#"{"sessionID":"ses-1","title":"Root","sneaky":1}"#,
        );
        assert!(parse_snapshot(&value).is_err());
    }

    #[test]
    fn opencode_tabs_rejects_bad_owner_sessions_titles_and_counts() {
        assert!(parse_snapshot(&snapshot_json("NOTHEX", "[]")).is_err());
        assert!(parse_snapshot(&snapshot_json(owner(), r#"{"sessionID":"","title":""}"#)).is_err());
        assert!(parse_snapshot(&snapshot_json(
            owner(),
            r#"{"sessionID":"bad id","title":""}"#
        ))
        .is_err());
        assert!(parse_snapshot(&snapshot_json(
            owner(),
            r#"{"sessionID":"ok","title":"a\nb"}"#
        ))
        .is_err());
        let too_many = (0..=MAX_TABS)
            .map(|index| format!(r#"{{"sessionID":"s{index}","title":""}}"#))
            .collect::<Vec<_>>()
            .join(",");
        assert!(parse_snapshot(&snapshot_json(owner(), &too_many)).is_err());
        let two_active = r#"{"sessionID":"a","title":"","active":true},{"sessionID":"b","title":"","active":true}"#;
        assert!(parse_snapshot(&snapshot_json(owner(), two_active)).is_err());
        let duplicate = r#"{"sessionID":"a","title":""},{"sessionID":"a","title":""}"#;
        assert!(parse_snapshot(&snapshot_json(owner(), duplicate)).is_err());
    }

    #[test]
    fn opencode_tabs_rejects_oversized_payload_and_version() {
        let big = "x".repeat(MAX_SNAPSHOT_BYTES + 1);
        assert!(parse_snapshot(&big).is_err());
        let wrong_version = format!(r#"{{"v":2,"owner":"{}","tabs":[]}}"#, owner());
        assert!(parse_snapshot(&wrong_version).is_err());
    }

    #[test]
    fn opencode_tabs_encoded_snapshot_round_trips_with_expiry() {
        let value = snapshot_json(owner(), r#"{"sessionID":"ses","title":"T"}"#);
        let mut parsed = parse_snapshot(&value).unwrap();
        parsed.expires_at_ms = Some(1234);
        let encoded = encode_snapshot(&parsed);
        assert!(encoded.contains("expiresAtMs"));
        let reparsed = parse_snapshot(&encoded).unwrap();
        assert_eq!(reparsed, parsed);
        assert!(snapshot_is_live(&reparsed, 1233));
        assert!(!snapshot_is_live(&reparsed, 1234));
    }

    #[test]
    fn opencode_tabs_focus_request_parses_and_validates() {
        let value = format!(
            r#"{{"v":1,"owner":"{}","sessionID":"ses-1","requestID":"{}"}}"#,
            owner(),
            "f".repeat(32)
        );
        let parsed = parse_focus_request(&value).unwrap();
        assert_eq!(parsed.session_id, "ses-1");
        assert_eq!(parsed.request_id, "f".repeat(32));
        assert_eq!(parsed.expires_at_ms, None);

        assert!(
            parse_focus_request(r#"{"v":1,"owner":"x","sessionID":"a","requestID":"b"}"#).is_err()
        );
        assert!(parse_focus_request(&format!(
            r#"{{"v":1,"owner":"{}","sessionID":"a","requestID":"{}","extra":1}}"#,
            owner(),
            "a".repeat(32)
        ))
        .is_err());
    }

    #[test]
    fn opencode_tabs_reserved_token_map_is_exclusive() {
        let tabs = HashMap::from([(TABS_TOKEN.to_string(), Some("{}".to_string()))]);
        assert_eq!(reserved_token_key(&tabs), Some(TABS_TOKEN));
        let focus = HashMap::from([(FOCUS_TOKEN.to_string(), Some("{}".to_string()))]);
        assert_eq!(reserved_token_key(&focus), Some(FOCUS_TOKEN));
        let ordinary = HashMap::from([("summary".to_string(), Some("x".to_string()))]);
        assert_eq!(reserved_token_key(&ordinary), None);
    }

    #[test]
    fn opencode_tabs_source_maps_are_fixed() {
        assert_eq!(source_for_token(TABS_TOKEN), Some(TABS_SOURCE));
        assert_eq!(source_for_token(FOCUS_TOKEN), Some(FOCUS_SOURCE));
        assert_eq!(source_for_token("summary"), None);
    }
}
