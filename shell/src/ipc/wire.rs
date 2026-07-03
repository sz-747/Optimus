//! Port of core/Ipc/WireProtocol.cs + SocketRequest.cs + Methods.cs — the newline-framed wire
//! shared by the CLI and the pipe server. V2 is newline-delimited JSON (`{id, method, params}` /
//! `{id, ok, result|error}`); a V1 text fallback (`verb args\n`) is still parsed because the CLI
//! round-trip tests exercise it. The parse boundary is typed (d30009a): malformed JSON and a
//! structurally-invalid request are distinct errors the router maps to `parse_error` /
//! `invalid_request`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Shared IPC method/verb constants (port of `SocketMethods`). External agents speak these over
/// the pipe — the strings are a contract and must not drift.
pub mod methods {
    // V2 system methods.
    pub const SYSTEM_PING: &str = "system.ping";
    pub const SYSTEM_CAPABILITIES: &str = "system.capabilities";

    // V1 verbs (space-separated, newline-framed).
    pub const SEND: &str = "send";
    pub const SEND_KEY: &str = "send-key";
    pub const NOTIFY: &str = "notify";
    pub const NOTIFY_TARGET: &str = "notify_target_async";
    pub const CREATE_NOTIFICATION: &str = "notification.create";
    pub const CREATE_NOTIFICATION_FOR_CALLER: &str = "notification.create_for_caller";
    pub const LIST_NOTIFICATIONS: &str = "notification.list";
    pub const DISMISS_NOTIFICATION: &str = "notification.dismiss";
    pub const DISMISS_NOTIFICATION_FOR_SURFACE: &str = "notification.dismiss_surface";
    pub const DISMISS_ALL_NOTIFICATIONS: &str = "notification.clear";
    pub const CLEAR_READ_NOTIFICATIONS: &str = "notification.mark_read";
    pub const OPEN_NOTIFICATION: &str = "notification.open";
    pub const JUMP_TO_UNREAD: &str = "notification.jump_to_unread";
    pub const SET_STATUS: &str = "set-status";
    pub const SET_PROGRESS: &str = "set-progress";
    pub const LOG_LINE: &str = "log";
    pub const SIDEBAR_STATE: &str = "sidebar-state";

    pub const REPORT_GIT_BRANCH: &str = "report_git_branch";
    pub const REPORT_PR: &str = "report_pr";
    pub const REPORT_PWD: &str = "report_pwd";
    pub const REPORT_SHELL_STATE: &str = "report_shell_state";
    pub const REPORT_REVIEW: &str = "report_review";

    // Events stream.
    pub const EVENTS_STREAM: &str = "events.stream";

    // V2 JSON methods.
    pub const SURFACE_SEND_TEXT: &str = "surface.send_text";
    pub const SURFACE_SEND_KEY: &str = "surface.send_key";
    pub const SURFACE_FOCUS: &str = "surface.focus";
    pub const AUTH_LOGIN: &str = "auth.login";

    // Authentication.
    pub const AUTH: &str = "auth";
}

const V2_SENTINEL: u8 = b'{';

/// A parsed V1 command (`verb args`). `args` is everything after the first space, verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V1Command {
    pub verb: String,
    pub args: String,
}

/// A parsed V2 request envelope. `params` defaults to an empty object when the field is absent.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct V2Request {
    pub id: String,
    pub method: String,
    pub params: Value,
}

/// A V2 error object (`{code, message}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct V2Error {
    pub code: String,
    pub message: String,
}

/// A V2 response envelope. Field order is fixed (`id, ok, result, error`) and nulls are emitted,
/// so the serialized bytes match the C# `V2Response` exactly (agents parse these).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V2Response {
    pub id: String,
    pub ok: bool,
    pub result: Option<Value>,
    pub error: Option<V2Error>,
}

/// A V2 notification frame for `events.stream`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V2Frame {
    pub method: String,
    pub params: Value,
}

/// Why a V2 request failed to parse. Maps to the wire error code the router replies with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// Not valid JSON — router replies `parse_error`.
    Malformed,
    /// Valid JSON but not a `{id: string, method: string, ...}` object — `invalid_request`.
    InvalidRequest,
}

/// True when the line is a V2 JSON frame (first non-newline char is `{`).
pub fn is_v2_frame(line: &str) -> bool {
    let clean = trim_trailing_newline(line);
    !clean.is_empty() && clean.as_bytes()[0] == V2_SENTINEL
}

/// Parse a V1 line into `verb` + everything-after-the-first-space `args`.
pub fn parse_v1(line: &str) -> V1Command {
    let clean = trim_trailing_newline(line);
    match clean.find(' ') {
        None => V1Command {
            verb: clean.to_string(),
            args: String::new(),
        },
        Some(idx) => V1Command {
            verb: clean[..idx].to_string(),
            args: clean[idx + 1..].to_string(),
        },
    }
}

/// Parse a V2 request. Malformed JSON and a structurally-invalid request are distinct errors.
pub fn parse_v2(line: &str) -> Result<V2Request, ParseError> {
    let clean = trim_trailing_newline(line);
    let value: Value = serde_json::from_str(clean).map_err(|_| ParseError::Malformed)?;
    let obj = value.as_object().ok_or(ParseError::InvalidRequest)?;
    let id = obj
        .get("id")
        .and_then(Value::as_str)
        .ok_or(ParseError::InvalidRequest)?;
    let method = obj
        .get("method")
        .and_then(Value::as_str)
        .ok_or(ParseError::InvalidRequest)?;
    let params = obj
        .get("params")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    Ok(V2Request {
        id: id.to_string(),
        method: method.to_string(),
        params,
    })
}

/// Serialize an OK response (`{id, ok:true, result, error:null}`) with the trailing newline.
pub fn serialize_ok(request_id: &str, result: Value) -> String {
    let response = V2Response {
        id: request_id.to_string(),
        ok: true,
        result: Some(result),
        error: None,
    };
    to_line(&response)
}

/// Serialize an error response (`{id, ok:false, result:null, error:{code, message}}`) + newline.
pub fn serialize_error(request_id: &str, code: &str, message: &str) -> String {
    let response = V2Response {
        id: request_id.to_string(),
        ok: false,
        result: None,
        error: Some(V2Error {
            code: code.to_string(),
            message: message.to_string(),
        }),
    };
    to_line(&response)
}

/// Frame a V1 text response with the trailing newline.
pub fn serialize_v1_response(payload: &str) -> String {
    format!("{payload}\n")
}

/// Split a buffer into newline-delimited frames, stripping a trailing `\r` from each and dropping
/// empty segments between delimiters. Faithful port of `SocketWireProtocol.SplitFrames`.
pub fn split_frames(buffer: &str) -> Vec<String> {
    let b = buffer.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    // Newline/CR are single ASCII bytes and never occur inside a UTF-8 multibyte sequence, so
    // splitting on byte offsets yields valid UTF-8 slices.
    for i in 0..=b.len() {
        if i < b.len() && b[i] != b'\n' {
            continue;
        }
        if i > start {
            let mut end = i;
            if b[end - 1] == b'\r' {
                end -= 1;
            }
            out.push(buffer[start..end].to_string());
        }
        start = i + 1;
    }
    out
}

fn to_line<T: Serialize>(value: &T) -> String {
    // serde_json emits struct fields in declaration order with no whitespace — matches C#'s
    // compact `JsonSerializer.Serialize`. Envelope types serialize infallibly.
    let mut s = serde_json::to_string(value).expect("envelope serialization is infallible");
    s.push('\n');
    s
}

fn trim_trailing_newline(text: &str) -> &str {
    match text.strip_suffix('\n') {
        Some(stripped) => stripped.strip_suffix('\r').unwrap_or(stripped),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detects_v2_when_line_starts_with_brace() {
        assert!(is_v2_frame("{\"id\":\"1\",\"method\":\"ping\"}\n"));
        assert!(!is_v2_frame("send hi\n"));
        assert!(!is_v2_frame(""));
    }

    #[test]
    fn parses_v1_line_by_single_split() {
        let parsed = parse_v1("notify target hello world");
        assert_eq!(parsed.verb, "notify");
        assert_eq!(parsed.args, "target hello world");
    }

    #[test]
    fn parses_v1_line_without_args() {
        let parsed = parse_v1("events.stream\n");
        assert_eq!(parsed.verb, "events.stream");
        assert_eq!(parsed.args, "");
    }

    #[test]
    fn splits_crlf_and_lf_frames() {
        let frames = split_frames("a\nb\r\nc\n");
        assert_eq!(frames, vec!["a", "b", "c"]);
    }

    #[test]
    fn parses_v2_json_payload() {
        let json = r#"{"id":"12","method":"notify","params":{"title":"done"}}"#;
        let req = parse_v2(json).unwrap();
        assert_eq!(req.id, "12");
        assert_eq!(req.method, "notify");
        assert_eq!(
            req.params.get("title").and_then(Value::as_str),
            Some("done")
        );
    }

    #[test]
    fn serializes_v2_ok_with_newline() {
        let response = serialize_ok("7", json!({"ok": true}));
        assert_eq!(
            response,
            "{\"id\":\"7\",\"ok\":true,\"result\":{\"ok\":true},\"error\":null}\n"
        );
    }

    // ---- Parse-boundary hardening (d30009a) — the typed distinction the router relies on. -------

    #[test]
    fn parse_v2_defaults_absent_params_to_empty_object() {
        let req = parse_v2(r#"{"id":"1","method":"system.ping"}"#).unwrap();
        assert_eq!(req.params, json!({}));
    }

    #[test]
    fn parse_v2_rejects_malformed_json_distinctly() {
        assert_eq!(parse_v2("{not json"), Err(ParseError::Malformed));
    }

    #[test]
    fn parse_v2_rejects_a_structurally_invalid_request() {
        assert_eq!(
            parse_v2(r#"["not","an","object"]"#),
            Err(ParseError::InvalidRequest)
        );
        assert_eq!(
            parse_v2(r#"{"method":"no-id"}"#),
            Err(ParseError::InvalidRequest)
        );
        assert_eq!(
            parse_v2(r#"{"id":42,"method":"nonstring-id"}"#),
            Err(ParseError::InvalidRequest)
        );
    }

    #[test]
    fn serialize_error_frames_code_and_message() {
        let line = serialize_error("9", "invalid_params", "bad");
        assert_eq!(
            line,
            "{\"id\":\"9\",\"ok\":false,\"result\":null,\"error\":{\"code\":\"invalid_params\",\"message\":\"bad\"}}\n"
        );
    }
}
