//! Port of core/Ipc/CommandRouter.cs + ISocketEffects.cs — pure socket dispatch for the Phase-4 IPC
//! contract. Parse one framed line, validate against auth state, run the mapped effect, return one
//! framed response (or `None` for the `events.stream` handoff signal). The parse boundary is typed
//! (d30009a): a JSON `null` or missing required field yields `invalid_params` and never reaches an
//! effect; malformed/structurally-invalid V2 collapses to `parse_error` (id "0"), matching C#.
//!
//! Divergence from C#: notification ids are the domain's `NotificationId(u64)` (a P2 decision),
//! not `Guid`. The wire parses a `u64` id; no C# router test exercised the id path.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::domain::ids::SurfaceId;
use crate::domain::notifications::{NotificationId, TerminalNotification};
use crate::ipc::access::AuthState;
use crate::ipc::wire::{self, methods, V1Command, V2Request};

const PARSE_ERROR_CODE: &str = "parse_error";
const METHOD_NOT_FOUND_CODE: &str = "method_not_found";
const INVALID_PARAMS_CODE: &str = "invalid_params";
const AUTH_REQUIRED_CODE: &str = "auth_required";
const CALLER_UNAUTHORIZED_CODE: &str = "caller_unauthorized";
const INTERNAL_ERROR_CODE: &str = "internal_error";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SocketEffectError {
    Unauthorized,
    InvalidInput(String),
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryRecordCommand {
    pub surface: SurfaceId,
    pub caller_capability: String,
    pub fact: String,
    pub why: Option<String>,
    pub kind: String,
    pub file_key: Option<String>,
    pub reverses: Option<i64>,
    pub defines: Vec<String>,
    pub references: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryDumpCommand {
    pub surface: SurfaceId,
    pub caller_capability: String,
    pub limit: usize,
    pub before_id: Option<i64>,
}

/// Socket-command effect surface (Core/U4): handlers call this instead of touching the UI, so
/// dispatch stays fully unit-testable. Methods take `&self`; a real implementation uses interior
/// mutability (the domain behind a lock). This mirrors the C# `ISocketEffects` minus the three
/// members the router never calls (`Tree`/`Ping`/`CreateNotification`) — add them when a consumer
/// beyond the router needs them.
pub trait SocketEffects {
    fn capabilities(&self) -> String;
    fn authenticate(&self, credential: &str) -> bool;

    fn focus_surface(&self, surface: SurfaceId);
    fn send_text(&self, surface: SurfaceId, text: &str);
    fn send_key(&self, surface: SurfaceId, virtual_key: u32, modifiers: u32);

    fn create_notification_for_target(
        &self,
        workspace_id: &str,
        surface: SurfaceId,
        title: &str,
        subtitle: &str,
        body: &str,
    );
    fn create_notification_for_caller(
        &self,
        preferred_surface_id: Option<&str>,
        title: &str,
        subtitle: &str,
        body: &str,
    );

    fn notification_list(&self) -> Vec<TerminalNotification>;
    fn notification_dismiss(&self, id: NotificationId);
    fn notification_dismiss_for_surface(&self, surface: SurfaceId);
    fn notification_dismiss_all_read(&self);
    fn notification_clear(&self);
    fn notification_mark_read(&self, id: NotificationId);
    fn notification_mark_read_for_surface(&self, surface: SurfaceId);
    fn notification_mark_all_read(&self);
    fn notification_open(&self, id: NotificationId);
    fn jump_to_unread(&self) -> bool;

    fn set_status(&self, surface: Option<SurfaceId>, status: &str);
    fn set_progress(&self, surface: Option<SurfaceId>, progress: &str);
    fn set_status_authenticated(
        &self,
        surface: Option<SurfaceId>,
        status: &str,
        _caller_capability: Option<&str>,
    ) -> Result<(), SocketEffectError> {
        self.set_status(surface, status);
        Ok(())
    }
    fn set_progress_authenticated(
        &self,
        surface: Option<SurfaceId>,
        progress: &str,
        _caller_capability: Option<&str>,
    ) -> Result<(), SocketEffectError> {
        self.set_progress(surface, progress);
        Ok(())
    }
    fn memory_record(&self, _command: MemoryRecordCommand) -> Result<Value, SocketEffectError> {
        Err(SocketEffectError::Internal)
    }
    fn memory_dump(&self, _command: MemoryDumpCommand) -> Result<Value, SocketEffectError> {
        Err(SocketEffectError::Internal)
    }
    fn log_line(&self, line: &str);
    fn sidebar_state(&self, payload: &str);

    fn report_git_branch(&self, surface: SurfaceId, branch: &str, is_dirty: bool);
    fn report_pr(
        &self,
        surface: SurfaceId,
        number: &str,
        label: &str,
        status: &str,
        branch: Option<&str>,
        is_stale: bool,
    );
    fn report_pwd(&self, surface: SurfaceId, path: &str);
}

/// Dispatch a raw framed line. `None` means "no inline response" — the caller upgrades the
/// connection to the events stream.
pub fn dispatch(line: &str, effects: &dyn SocketEffects, auth: AuthState) -> Option<String> {
    if line.trim().is_empty() {
        return Some(wire::serialize_v1_response("ERROR: empty request"));
    }

    if wire::is_v2_frame(line) {
        return match wire::parse_v2(line) {
            Ok(request) => dispatch_v2(&request, effects, auth),
            Err(_) => Some(wire::serialize_error(
                "0",
                PARSE_ERROR_CODE,
                "Invalid V2 request.",
            )),
        };
    }

    dispatch_v1(&wire::parse_v1(line), effects, auth)
}

fn dispatch_v1(
    command: &V1Command,
    effects: &dyn SocketEffects,
    auth: AuthState,
) -> Option<String> {
    if command.verb == methods::EVENTS_STREAM {
        return None;
    }

    if is_auth_required(&command.verb, auth) {
        return Some(wire::serialize_v1_response("ERROR: auth required"));
    }

    let args = command.args.as_str();
    let response = match command.verb.as_str() {
        methods::SEND => handle_send_text(args, effects),
        methods::SEND_KEY => handle_send_key(args, effects),
        methods::NOTIFY | methods::NOTIFY_TARGET => handle_notify_target(args, effects),
        methods::CREATE_NOTIFICATION => handle_create_notification(args, effects),
        methods::CREATE_NOTIFICATION_FOR_CALLER => handle_create_for_caller(args, effects),
        methods::LIST_NOTIFICATIONS => handle_list_notifications(effects),
        methods::DISMISS_NOTIFICATION => handle_notification_dismiss(args, effects),
        methods::DISMISS_NOTIFICATION_FOR_SURFACE => {
            handle_notification_dismiss_for_surface(args, effects)
        }
        methods::DISMISS_ALL_NOTIFICATIONS => handle_notification_clear(effects),
        methods::CLEAR_READ_NOTIFICATIONS => handle_notification_mark_read(args, effects),
        methods::OPEN_NOTIFICATION => handle_notification_open(args, effects),
        methods::JUMP_TO_UNREAD => handle_jump_to_unread(effects),
        methods::SET_STATUS => handle_set_status(args, effects),
        methods::SET_PROGRESS => handle_set_progress(args, effects),
        methods::LOG_LINE => handle_log(args, effects),
        methods::SIDEBAR_STATE => handle_sidebar_state(args, effects),
        methods::AUTH => handle_auth(args, effects),
        methods::REPORT_GIT_BRANCH => handle_report_git_branch(args, effects),
        methods::REPORT_PR => handle_report_pr(args, effects),
        methods::REPORT_PWD => handle_report_pwd(args, effects),
        other => wire::serialize_v1_response(&format!("ERROR: unknown command \"{other}\"")),
    };
    Some(response)
}

fn dispatch_v2(
    request: &V2Request,
    effects: &dyn SocketEffects,
    auth: AuthState,
) -> Option<String> {
    if request.method == methods::EVENTS_STREAM {
        return None;
    }

    if is_auth_required(&request.method, auth) {
        return Some(wire::serialize_error(
            &request.id,
            AUTH_REQUIRED_CODE,
            "Authentication required.",
        ));
    }

    let response = match request.method.as_str() {
        methods::SYSTEM_PING => ok(request, json!({ "pong": true })),
        methods::SYSTEM_CAPABILITIES => {
            ok(request, json!({ "capabilities": effects.capabilities() }))
        }
        methods::NOTIFY => handle_notify_v2(request, effects),
        methods::CREATE_NOTIFICATION_FOR_CALLER => handle_create_for_caller_v2(request, effects),
        methods::AUTH_LOGIN => handle_auth_login(request, effects),
        methods::SURFACE_SEND_TEXT => handle_surface_send_text(request, effects),
        methods::SURFACE_SEND_KEY => handle_surface_send_key(request, effects),
        methods::SET_STATUS => handle_set_status_v2(request, effects),
        methods::SET_PROGRESS => handle_set_progress_v2(request, effects),
        methods::MEMORY_RECORD => handle_memory_record_v2(request, effects),
        methods::MEMORY_DUMP => handle_memory_dump_v2(request, effects),
        methods::LOG_LINE => handle_log_v2(request, effects),
        methods::SIDEBAR_STATE => handle_sidebar_state_v2(request, effects),
        methods::LIST_NOTIFICATIONS => ok(
            request,
            json!({ "notifications": serialize_notifications(&effects.notification_list()) }),
        ),
        methods::DISMISS_ALL_NOTIFICATIONS => {
            effects.notification_clear();
            ok(request, json!({ "ok": true }))
        }
        methods::DISMISS_NOTIFICATION => handle_notification_dismiss_v2(request, effects),
        methods::DISMISS_NOTIFICATION_FOR_SURFACE => {
            handle_notification_dismiss_for_surface_v2(request, effects)
        }
        methods::CLEAR_READ_NOTIFICATIONS => handle_notification_mark_read_v2(request, effects),
        methods::OPEN_NOTIFICATION => handle_notification_open_v2(request, effects),
        methods::JUMP_TO_UNREAD => ok(request, json!({ "jumped": effects.jump_to_unread() })),
        methods::REPORT_GIT_BRANCH => handle_report_git_branch_v2(request, effects),
        methods::REPORT_PR => handle_report_pr_v2(request, effects),
        methods::REPORT_PWD => handle_report_pwd_v2(request, effects),
        methods::SURFACE_FOCUS => handle_surface_focus_v2(request, effects),
        _ => err(request, METHOD_NOT_FOUND_CODE, "Unknown method."),
    };
    Some(response)
}

// ---- V1 handlers ------------------------------------------------------------------------------

fn handle_send_text(args: &str, effects: &dyn SocketEffects) -> String {
    let Some((_, surface, payload)) = parse_workspace_surface_and_remainder(args) else {
        return wire::serialize_v1_response("ERROR: expected: send <surface> <text>");
    };
    effects.send_text(surface, &payload);
    wire::serialize_v1_response("OK")
}

fn handle_send_key(args: &str, effects: &dyn SocketEffects) -> String {
    let Some((_, surface, payload)) = parse_workspace_surface_and_remainder(args) else {
        return wire::serialize_v1_response(
            "ERROR: expected: send-key <surface> <key> [modifiers]",
        );
    };
    let parts: Vec<&str> = payload.split(' ').filter(|s| !s.is_empty()).collect();
    let Some(key) = parts.first().and_then(|s| s.parse::<u32>().ok()) else {
        return wire::serialize_v1_response("ERROR: invalid key.");
    };
    let modifiers = match parts.get(1) {
        Some(m) => match m.parse::<u32>() {
            Ok(v) => v,
            Err(_) => return wire::serialize_v1_response("ERROR: invalid modifiers."),
        },
        None => 0,
    };
    effects.send_key(surface, key, modifiers);
    wire::serialize_v1_response("OK")
}

fn handle_notify_target(args: &str, effects: &dyn SocketEffects) -> String {
    let Some((workspace, surface, body)) = parse_workspace_surface_and_remainder(args) else {
        return wire::serialize_v1_response(
            "ERROR: expected: notify_target_async <workspace> <surface> <title>|<subtitle>|<body>",
        );
    };
    let (title, subtitle, message) = split_pipe3(&body);
    effects.create_notification_for_target(
        workspace.as_deref().unwrap_or(""),
        surface,
        &title,
        &subtitle,
        &message,
    );
    wire::serialize_v1_response("OK")
}

fn handle_create_notification(args: &str, effects: &dyn SocketEffects) -> String {
    let Some((workspace, surface, body)) = parse_workspace_surface_and_remainder(args) else {
        return wire::serialize_v1_response(
            "ERROR: expected: notification.create <workspace> <surface> <title>|<subtitle>|<body>",
        );
    };
    let (title, subtitle, message) = split_pipe3(&body);
    effects.create_notification_for_target(
        workspace.as_deref().unwrap_or(""),
        surface,
        &title,
        &subtitle,
        &message,
    );
    wire::serialize_v1_response("OK")
}

fn handle_create_for_caller(args: &str, effects: &dyn SocketEffects) -> String {
    let (title, subtitle, body) = split_pipe3(args);
    effects.create_notification_for_caller(None, &title, &subtitle, &body);
    wire::serialize_v1_response("OK")
}

fn handle_list_notifications(effects: &dyn SocketEffects) -> String {
    let count = effects.notification_list().len();
    wire::serialize_v1_response(&format!("OK: {count} notifications"))
}

fn handle_notification_dismiss(args: &str, effects: &dyn SocketEffects) -> String {
    if let Some(id) = parse_notification_id(args) {
        effects.notification_dismiss(id);
        return wire::serialize_v1_response("OK");
    }
    if let Some(surface) = parse_surface(args) {
        effects.notification_dismiss_for_surface(surface);
        return wire::serialize_v1_response("OK");
    }
    if is_all_scope(args) {
        effects.notification_dismiss_all_read();
        return wire::serialize_v1_response("OK");
    }
    wire::serialize_v1_response("ERROR: expected notification id, surface id, or all_read.")
}

fn handle_notification_dismiss_for_surface(args: &str, effects: &dyn SocketEffects) -> String {
    let Some(surface) = parse_surface(args) else {
        return wire::serialize_v1_response("ERROR: expected notification surface id.");
    };
    effects.notification_dismiss_for_surface(surface);
    wire::serialize_v1_response("OK")
}

fn handle_notification_clear(effects: &dyn SocketEffects) -> String {
    effects.notification_clear();
    wire::serialize_v1_response("OK")
}

fn handle_notification_mark_read(args: &str, effects: &dyn SocketEffects) -> String {
    if let Some(id) = parse_notification_id(args) {
        effects.notification_mark_read(id);
        return wire::serialize_v1_response("OK");
    }
    if let Some(surface) = parse_surface(args) {
        effects.notification_mark_read_for_surface(surface);
        return wire::serialize_v1_response("OK");
    }
    if is_all_scope(args) {
        effects.notification_mark_all_read();
        return wire::serialize_v1_response("OK");
    }
    wire::serialize_v1_response("ERROR: expected notification id, surface id, or all.")
}

fn handle_notification_open(args: &str, effects: &dyn SocketEffects) -> String {
    let Some(id) = parse_notification_id(args) else {
        return wire::serialize_v1_response("ERROR: expected notification id.");
    };
    effects.notification_open(id);
    wire::serialize_v1_response("OK")
}

fn handle_jump_to_unread(effects: &dyn SocketEffects) -> String {
    if effects.jump_to_unread() {
        wire::serialize_v1_response("OK")
    } else {
        wire::serialize_v1_response("ERROR: no unread notification")
    }
}

fn handle_set_status(args: &str, effects: &dyn SocketEffects) -> String {
    effects.set_status(None, args);
    wire::serialize_v1_response("OK")
}

fn handle_set_progress(args: &str, effects: &dyn SocketEffects) -> String {
    effects.set_progress(None, args);
    wire::serialize_v1_response("OK")
}

fn handle_log(args: &str, effects: &dyn SocketEffects) -> String {
    effects.log_line(args);
    wire::serialize_v1_response("OK")
}

fn handle_sidebar_state(args: &str, effects: &dyn SocketEffects) -> String {
    effects.sidebar_state(args);
    wire::serialize_v1_response("OK")
}

fn handle_auth(args: &str, effects: &dyn SocketEffects) -> String {
    let ok = effects.authenticate(args);
    wire::serialize_v1_response(if ok { "OK: auth" } else { "ERROR: auth failed" })
}

fn handle_report_git_branch(args: &str, effects: &dyn SocketEffects) -> String {
    let Some((_, surface, payload)) = parse_workspace_surface_and_remainder(args) else {
        return wire::serialize_v1_response(
            "ERROR: expected: report_git_branch <workspace> <surface> <branch>|<isDirty>",
        );
    };
    let parts: Vec<&str> = payload.splitn(3, '|').collect();
    if parts.len() < 2 {
        return wire::serialize_v1_response("ERROR: expected: <branch>|<isDirty>");
    }
    let Some(is_dirty) = parse_bool(parts[1]) else {
        return wire::serialize_v1_response("ERROR: expected isDirty bool.");
    };
    effects.report_git_branch(surface, parts[0], is_dirty);
    wire::serialize_v1_response("OK")
}

fn handle_report_pr(args: &str, effects: &dyn SocketEffects) -> String {
    let Some((_, surface, payload)) = parse_workspace_surface_and_remainder(args) else {
        return wire::serialize_v1_response(
            "ERROR: expected: report_pr <workspace> <surface> <num>|<label>|<url>|<status>|<branch>|<isStale>",
        );
    };
    let parts: Vec<&str> = payload.splitn(6, '|').collect();
    if parts.len() < 4 {
        return wire::serialize_v1_response(
            "ERROR: expected: <num>|<label>|<url>|<status>|<branch>|<isStale>",
        );
    }
    let number = parts[0];
    let label = parts[1];
    // parts[2] = url, parsed but dropped (the Rust PullRequestInfo has no url field).
    let status = parts[3];
    let branch = parts.get(4).filter(|b| !b.is_empty()).copied();
    let is_stale = parts.get(5).and_then(|s| parse_bool(s)).unwrap_or(false);
    effects.report_pr(surface, number, label, status, branch, is_stale);
    wire::serialize_v1_response("OK")
}

fn handle_report_pwd(args: &str, effects: &dyn SocketEffects) -> String {
    let Some((_, surface, payload)) = parse_workspace_surface_and_remainder(args) else {
        return wire::serialize_v1_response(
            "ERROR: expected: report_pwd <workspace> <surface> <path>",
        );
    };
    effects.report_pwd(surface, &payload);
    wire::serialize_v1_response("OK")
}

// ---- V2 handlers ------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryRecordParams {
    surface_id: String,
    caller_capability: String,
    fact: String,
    why: Option<String>,
    kind: String,
    file_key: Option<String>,
    reverses: Option<i64>,
    #[serde(default)]
    defines: Vec<String>,
    #[serde(default)]
    references: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryDumpParams {
    surface_id: String,
    caller_capability: String,
    limit: Option<usize>,
    before_id: Option<i64>,
}

fn handle_notify_v2(request: &V2Request, effects: &dyn SocketEffects) -> String {
    let Some(surface) = surface_from_params(&request.params) else {
        return err(request, INVALID_PARAMS_CODE, "Missing surface_id.");
    };
    let title = string_param(&request.params, "title").unwrap_or_default();
    let subtitle = string_param(&request.params, "subtitle").unwrap_or_default();
    let body = string_param(&request.params, "body").unwrap_or_default();
    let workspace = string_param(&request.params, "workspace_id").unwrap_or_default();
    effects.create_notification_for_target(&workspace, surface, &title, &subtitle, &body);
    ok(request, json!({ "ok": true }))
}

fn handle_create_for_caller_v2(request: &V2Request, effects: &dyn SocketEffects) -> String {
    let title = string_param(&request.params, "title").unwrap_or_default();
    let subtitle = string_param(&request.params, "subtitle").unwrap_or_default();
    let body = string_param(&request.params, "body").unwrap_or_default();
    let preferred = string_param(&request.params, "preferred_surface_id");
    effects.create_notification_for_caller(preferred.as_deref(), &title, &subtitle, &body);
    ok(request, json!({ "ok": true }))
}

fn handle_auth_login(request: &V2Request, effects: &dyn SocketEffects) -> String {
    let credential = string_param(&request.params, "credential")
        .or_else(|| string_param(&request.params, "password"))
        .unwrap_or_default();
    let authorized = effects.authenticate(&credential);
    ok(request, json!({ "authorized": authorized }))
}

fn handle_surface_send_text(request: &V2Request, effects: &dyn SocketEffects) -> String {
    let Some(surface) = surface_from_params(&request.params) else {
        return err(request, INVALID_PARAMS_CODE, "Missing surface_id.");
    };
    let Some(text) = string_param(&request.params, "text") else {
        return err(request, INVALID_PARAMS_CODE, "Missing text.");
    };
    effects.send_text(surface, &text);
    ok(request, json!({ "ok": true }))
}

fn handle_surface_send_key(request: &V2Request, effects: &dyn SocketEffects) -> String {
    let Some(surface) = surface_from_params(&request.params) else {
        return err(request, INVALID_PARAMS_CODE, "Missing surface_id.");
    };
    let Some(key) = uint_param(&request.params, "key") else {
        return err(request, INVALID_PARAMS_CODE, "Missing key.");
    };
    let modifiers = uint_param(&request.params, "modifiers")
        .or_else(|| uint_param(&request.params, "modifier"))
        .unwrap_or(0);
    effects.send_key(surface, key, modifiers);
    ok(request, json!({ "ok": true }))
}

fn handle_surface_focus_v2(request: &V2Request, effects: &dyn SocketEffects) -> String {
    let Some(surface) = surface_from_params(&request.params) else {
        return err(request, INVALID_PARAMS_CODE, "Missing surface_id.");
    };
    effects.focus_surface(surface);
    ok(request, json!({ "ok": true }))
}

fn handle_set_status_v2(request: &V2Request, effects: &dyn SocketEffects) -> String {
    let Some(status) = string_param(&request.params, "status") else {
        return err(request, INVALID_PARAMS_CODE, "Missing status.");
    };
    let surface = match optional_surface_param(&request.params) {
        Ok(surface) => surface,
        Err(()) => return err(request, INVALID_PARAMS_CODE, "Invalid surface_id."),
    };
    let capability = string_param(&request.params, "caller_capability");
    match effects.set_status_authenticated(surface, &status, capability.as_deref()) {
        Ok(()) => ok(request, json!({ "ok": true })),
        Err(error) => effect_error(request, error),
    }
}

fn handle_set_progress_v2(request: &V2Request, effects: &dyn SocketEffects) -> String {
    let Some(progress) = string_param(&request.params, "progress") else {
        return err(request, INVALID_PARAMS_CODE, "Missing progress.");
    };
    let surface = match optional_surface_param(&request.params) {
        Ok(surface) => surface,
        Err(()) => return err(request, INVALID_PARAMS_CODE, "Invalid surface_id."),
    };
    let capability = string_param(&request.params, "caller_capability");
    match effects.set_progress_authenticated(surface, &progress, capability.as_deref()) {
        Ok(()) => ok(request, json!({ "ok": true })),
        Err(error) => effect_error(request, error),
    }
}

fn handle_memory_record_v2(request: &V2Request, effects: &dyn SocketEffects) -> String {
    let params = match serde_json::from_value::<MemoryRecordParams>(request.params.clone()) {
        Ok(params) => params,
        Err(_) => {
            return err(
                request,
                INVALID_PARAMS_CODE,
                "Invalid memory.record parameters.",
            )
        }
    };
    let Some(surface) = parse_surface_id(&params.surface_id) else {
        return err(request, INVALID_PARAMS_CODE, "Invalid surface_id.");
    };
    if params.caller_capability.is_empty()
        || params.fact.is_empty()
        || params.reverses.is_some_and(|value| value <= 0)
    {
        return err(
            request,
            INVALID_PARAMS_CODE,
            "Invalid memory.record parameters.",
        );
    }
    let command = MemoryRecordCommand {
        surface,
        caller_capability: params.caller_capability,
        fact: params.fact,
        why: params.why,
        kind: params.kind,
        file_key: params.file_key,
        reverses: params.reverses,
        defines: params.defines,
        references: params.references,
    };
    match effects.memory_record(command) {
        Ok(result) => ok(request, result),
        Err(error) => effect_error(request, error),
    }
}

fn handle_memory_dump_v2(request: &V2Request, effects: &dyn SocketEffects) -> String {
    let params = match serde_json::from_value::<MemoryDumpParams>(request.params.clone()) {
        Ok(params) => params,
        Err(_) => {
            return err(
                request,
                INVALID_PARAMS_CODE,
                "Invalid memory.dump parameters.",
            )
        }
    };
    let Some(surface) = parse_surface_id(&params.surface_id) else {
        return err(request, INVALID_PARAMS_CODE, "Invalid surface_id.");
    };
    if params.caller_capability.is_empty() || params.before_id.is_some_and(|value| value <= 0) {
        return err(
            request,
            INVALID_PARAMS_CODE,
            "Invalid memory.dump parameters.",
        );
    }
    let command = MemoryDumpCommand {
        surface,
        caller_capability: params.caller_capability,
        limit: params.limit.unwrap_or(100).clamp(1, 500),
        before_id: params.before_id,
    };
    match effects.memory_dump(command) {
        Ok(result) => ok(request, result),
        Err(error) => effect_error(request, error),
    }
}

fn effect_error(request: &V2Request, error: SocketEffectError) -> String {
    match error {
        SocketEffectError::Unauthorized => err(
            request,
            CALLER_UNAUTHORIZED_CODE,
            "Caller capability is not authorized.",
        ),
        SocketEffectError::InvalidInput(message) => err(request, INVALID_PARAMS_CODE, &message),
        SocketEffectError::Internal => {
            err(request, INTERNAL_ERROR_CODE, "The memory operation failed.")
        }
    }
}

fn handle_log_v2(request: &V2Request, effects: &dyn SocketEffects) -> String {
    let message =
        string_param(&request.params, "message").unwrap_or_else(|| raw_text(&request.params));
    effects.log_line(&message);
    ok(request, json!({ "ok": true }))
}

fn handle_sidebar_state_v2(request: &V2Request, effects: &dyn SocketEffects) -> String {
    effects.sidebar_state(&raw_text(&request.params));
    ok(request, json!({ "ok": true }))
}

fn handle_notification_dismiss_v2(request: &V2Request, effects: &dyn SocketEffects) -> String {
    if let Some(id) = notification_id_param(&request.params) {
        effects.notification_dismiss(id);
        return ok(request, json!({ "ok": true }));
    }
    if let Some(surface) = surface_from_params(&request.params) {
        effects.notification_dismiss_for_surface(surface);
        return ok(request, json!({ "ok": true }));
    }
    if notification_scope_param(&request.params)
        .map(|s| is_all_scope(&s))
        .unwrap_or(false)
    {
        effects.notification_dismiss_all_read();
        return ok(request, json!({ "ok": true }));
    }
    err(
        request,
        INVALID_PARAMS_CODE,
        "Missing id, surface_id, or all_read.",
    )
}

fn handle_notification_dismiss_for_surface_v2(
    request: &V2Request,
    effects: &dyn SocketEffects,
) -> String {
    let Some(surface) = surface_from_params(&request.params) else {
        return err(request, INVALID_PARAMS_CODE, "Missing surface_id.");
    };
    effects.notification_dismiss_for_surface(surface);
    ok(request, json!({ "ok": true }))
}

fn handle_notification_mark_read_v2(request: &V2Request, effects: &dyn SocketEffects) -> String {
    if let Some(id) = notification_id_param(&request.params) {
        effects.notification_mark_read(id);
        return ok(request, json!({ "ok": true }));
    }
    if let Some(surface) = surface_from_params(&request.params) {
        effects.notification_mark_read_for_surface(surface);
        return ok(request, json!({ "ok": true }));
    }
    if notification_scope_param(&request.params)
        .map(|s| is_all_scope(&s))
        .unwrap_or(false)
    {
        effects.notification_mark_all_read();
        return ok(request, json!({ "ok": true }));
    }
    err(
        request,
        INVALID_PARAMS_CODE,
        "Missing id, surface_id, or all.",
    )
}

fn handle_notification_open_v2(request: &V2Request, effects: &dyn SocketEffects) -> String {
    let Some(id) = notification_id_param(&request.params) else {
        return err(request, INVALID_PARAMS_CODE, "Missing id.");
    };
    effects.notification_open(id);
    ok(request, json!({ "ok": true }))
}

fn handle_report_git_branch_v2(request: &V2Request, effects: &dyn SocketEffects) -> String {
    let Some(surface) = surface_from_params(&request.params) else {
        return err(request, INVALID_PARAMS_CODE, "Missing surface_id.");
    };
    let branch = string_param(&request.params, "branch").unwrap_or_default();
    let Some(is_dirty) = bool_param(&request.params, "is_dirty") else {
        return err(request, INVALID_PARAMS_CODE, "Missing is_dirty.");
    };
    effects.report_git_branch(surface, &branch, is_dirty);
    ok(request, json!({ "ok": true }))
}

fn handle_report_pr_v2(request: &V2Request, effects: &dyn SocketEffects) -> String {
    let Some(surface) = surface_from_params(&request.params) else {
        return err(request, INVALID_PARAMS_CODE, "Missing surface_id.");
    };
    let number = string_param(&request.params, "number").unwrap_or_default();
    let label = string_param(&request.params, "label").unwrap_or_default();
    // "url" is accepted on the wire but dropped (no url field on the Rust PR record).
    let status = string_param(&request.params, "status").unwrap_or_default();
    let branch = string_param(&request.params, "branch");
    let is_stale = bool_param(&request.params, "is_stale").unwrap_or(false);
    effects.report_pr(
        surface,
        &number,
        &label,
        &status,
        branch.as_deref(),
        is_stale,
    );
    ok(request, json!({ "ok": true }))
}

fn handle_report_pwd_v2(request: &V2Request, effects: &dyn SocketEffects) -> String {
    let Some(surface) = surface_from_params(&request.params) else {
        return err(request, INVALID_PARAMS_CODE, "Missing surface_id.");
    };
    let Some(path) = string_param(&request.params, "path") else {
        return err(request, INVALID_PARAMS_CODE, "Missing path.");
    };
    effects.report_pwd(surface, &path);
    ok(request, json!({ "ok": true }))
}

// ---- Shared helpers ---------------------------------------------------------------------------

fn is_auth_required(method: &str, auth: AuthState) -> bool {
    auth.requires_authentication
        && !auth.is_authenticated
        && method != methods::AUTH
        && method != methods::AUTH_LOGIN
}

fn ok(request: &V2Request, result: Value) -> String {
    wire::serialize_ok(&request.id, result)
}

fn err(request: &V2Request, code: &str, message: &str) -> String {
    wire::serialize_error(&request.id, code, message)
}

fn raw_text(params: &Value) -> String {
    serde_json::to_string(params).unwrap_or_default()
}

fn serialize_notifications(notifications: &[TerminalNotification]) -> Vec<Value> {
    notifications
        .iter()
        .map(|n| {
            json!({
                "id": n.id.0,
                "surface": n.surface_id.to_string(),
                "title": n.title,
                "subtitle": n.subtitle,
                "body": n.body,
                "is_read": n.is_read,
                "pane_flash": n.pane_flash,
            })
        })
        .collect()
}

/// Split a pipe-delimited body into (title, subtitle, body); missing fields degrade to "".
fn split_pipe3(text: &str) -> (String, String, String) {
    let parts: Vec<&str> = text.splitn(3, '|').collect();
    (
        parts.first().copied().unwrap_or("").to_string(),
        parts.get(1).copied().unwrap_or("").to_string(),
        parts.get(2).copied().unwrap_or("").to_string(),
    )
}

/// `.NET bool.TryParse` semantics: case-insensitive `true`/`false` with surrounding whitespace.
fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn is_all_scope(args: &str) -> bool {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return false;
    }
    let scope = match trimmed.find(' ') {
        Some(i) => &trimmed[..i],
        None => trimmed,
    };
    scope.eq_ignore_ascii_case("all") || scope.eq_ignore_ascii_case("all_read")
}

/// Parse `[workspace] <surface> <remainder>`: if the first token is a surface, there's no
/// workspace; otherwise the first token is the workspace and the second must be the surface.
fn parse_workspace_surface_and_remainder(
    args: &str,
) -> Option<(Option<String>, SurfaceId, String)> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return None;
    }
    let first_space = trimmed.find(' ')?;
    let first = &trimmed[..first_space];
    let after_first = &trimmed[first_space + 1..];

    if let Some(surface) = parse_surface_id(first) {
        return Some((None, surface, after_first.to_string()));
    }

    let second_space = after_first.find(' ')?;
    let second = &after_first[..second_space];
    if let Some(surface) = parse_surface_id(second) {
        return Some((
            Some(first.to_string()),
            surface,
            after_first[second_space + 1..].to_string(),
        ));
    }

    None
}

fn parse_surface(args: &str) -> Option<SurfaceId> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(surface) = parse_surface_id(trimmed) {
        return Some(surface);
    }
    let space = trimmed.find(' ')?;
    parse_surface_id(trimmed[space + 1..].trim_start())
}

fn parse_notification_id(args: &str) -> Option<NotificationId> {
    let trimmed = args.trim();
    if let Ok(n) = trimmed.parse::<u64>() {
        return Some(NotificationId(n));
    }
    let space = trimmed.find(' ')?;
    trimmed[space + 1..]
        .trim_start()
        .parse::<u64>()
        .ok()
        .map(NotificationId)
}

/// Parse a surface id from `S<n>` or a bare integer.
fn parse_surface_id(value: &str) -> Option<SurfaceId> {
    if value.trim().is_empty() {
        return None;
    }
    let bytes = value.as_bytes();
    if bytes.len() > 1 && bytes[0].eq_ignore_ascii_case(&b'S') {
        if let Ok(raw) = value[1..].trim().parse::<i32>() {
            return Some(SurfaceId(raw));
        }
    }
    value.trim().parse::<i32>().ok().map(SurfaceId)
}

fn surface_from_params(params: &Value) -> Option<SurfaceId> {
    if let Some(text) = string_param(params, "surface_id") {
        if let Some(surface) = parse_surface_id(&text) {
            return Some(surface);
        }
    }
    params
        .as_object()?
        .get("surface")
        .and_then(Value::as_str)
        .and_then(parse_surface_id)
}

fn optional_surface_param(params: &Value) -> Result<Option<SurfaceId>, ()> {
    let Some(object) = params.as_object() else {
        return Ok(None);
    };
    let Some(raw) = object.get("surface_id").or_else(|| object.get("surface")) else {
        return Ok(None);
    };
    let value = raw.as_str().ok_or(())?;
    parse_surface_id(value).map(Some).ok_or(())
}

/// Port of `TryGetStringParam`: a JSON string (incl. empty) is taken verbatim; JSON `null` yields
/// `None` (the null-hardening); other scalars stringify.
fn string_param(params: &Value, name: &str) -> Option<String> {
    let raw = params.as_object()?.get(name)?;
    match raw {
        Value::String(s) => Some(s.clone()),
        Value::Null => None,
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        other => {
            let s = other.to_string();
            (!s.is_empty()).then_some(s)
        }
    }
}

fn bool_param(params: &Value, name: &str) -> Option<bool> {
    let raw = params.as_object()?.get(name)?;
    match raw {
        Value::Bool(b) => Some(*b),
        Value::String(s) => parse_bool(s),
        _ => None,
    }
}

fn uint_param(params: &Value, name: &str) -> Option<u32> {
    let raw = params.as_object()?.get(name)?;
    match raw {
        Value::Number(n) => n
            .as_u64()
            .filter(|v| *v <= u32::MAX as u64)
            .map(|v| v as u32),
        Value::String(s) => s.trim().parse::<u32>().ok(),
        _ => None,
    }
}

fn notification_id_param(params: &Value) -> Option<NotificationId> {
    let raw = params.as_object()?.get("id")?;
    match raw {
        Value::String(s) => s.trim().parse::<u64>().ok().map(NotificationId),
        Value::Number(n) => n.as_u64().map(NotificationId),
        _ => None,
    }
}

fn notification_scope_param(params: &Value) -> Option<String> {
    let scope = params.as_object()?.get("scope")?.as_str()?;
    (!scope.trim().is_empty()).then(|| scope.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    struct FakeSocketEffects {
        created_for_surface: Cell<Option<SurfaceId>>,
        created_for_workspace: RefCell<String>,
        created_title: RefCell<String>,
        created_subtitle: RefCell<String>,
        created_body: RefCell<String>,
        caller_preferred_surface: RefCell<Option<String>>,
        caller_title: RefCell<String>,
        caller_subtitle: RefCell<String>,
        caller_body: RefCell<String>,
        mark_read_surface: Cell<Option<SurfaceId>>,
        mark_all_read: Cell<bool>,
        dismiss_all_read_called: Cell<bool>,
        sent_surface: Cell<Option<SurfaceId>>,
        sent_text: RefCell<String>,
        memory_record: RefCell<Option<MemoryRecordCommand>>,
        memory_dump: RefCell<Option<MemoryDumpCommand>>,
    }

    impl SocketEffects for FakeSocketEffects {
        fn capabilities(&self) -> String {
            "v1,v2".to_string()
        }
        fn authenticate(&self, credential: &str) -> bool {
            credential == "ok"
        }
        fn focus_surface(&self, _surface: SurfaceId) {}
        fn send_text(&self, surface: SurfaceId, text: &str) {
            self.sent_surface.set(Some(surface));
            *self.sent_text.borrow_mut() = text.to_string();
        }
        fn send_key(&self, _surface: SurfaceId, _virtual_key: u32, _modifiers: u32) {}
        fn create_notification_for_target(
            &self,
            workspace_id: &str,
            surface: SurfaceId,
            title: &str,
            subtitle: &str,
            body: &str,
        ) {
            *self.created_for_workspace.borrow_mut() = workspace_id.to_string();
            self.created_for_surface.set(Some(surface));
            *self.created_title.borrow_mut() = title.to_string();
            *self.created_subtitle.borrow_mut() = subtitle.to_string();
            *self.created_body.borrow_mut() = body.to_string();
        }
        fn create_notification_for_caller(
            &self,
            preferred_surface_id: Option<&str>,
            title: &str,
            subtitle: &str,
            body: &str,
        ) {
            *self.caller_preferred_surface.borrow_mut() = preferred_surface_id.map(str::to_string);
            *self.caller_title.borrow_mut() = title.to_string();
            *self.caller_subtitle.borrow_mut() = subtitle.to_string();
            *self.caller_body.borrow_mut() = body.to_string();
        }
        fn notification_list(&self) -> Vec<TerminalNotification> {
            Vec::new()
        }
        fn notification_dismiss(&self, _id: NotificationId) {}
        fn notification_dismiss_for_surface(&self, _surface: SurfaceId) {}
        fn notification_dismiss_all_read(&self) {
            self.dismiss_all_read_called.set(true);
        }
        fn notification_clear(&self) {}
        fn notification_mark_read(&self, _id: NotificationId) {}
        fn notification_mark_read_for_surface(&self, surface: SurfaceId) {
            self.mark_read_surface.set(Some(surface));
        }
        fn notification_mark_all_read(&self) {
            self.mark_all_read.set(true);
        }
        fn notification_open(&self, _id: NotificationId) {}
        fn jump_to_unread(&self) -> bool {
            true
        }
        fn set_status(&self, _surface: Option<SurfaceId>, _status: &str) {}
        fn set_progress(&self, _surface: Option<SurfaceId>, _progress: &str) {}
        fn memory_record(&self, command: MemoryRecordCommand) -> Result<Value, SocketEffectError> {
            if command.caller_capability == "deny" {
                return Err(SocketEffectError::Unauthorized);
            }
            *self.memory_record.borrow_mut() = Some(command);
            Ok(json!({ "record_id": 91 }))
        }
        fn memory_dump(&self, command: MemoryDumpCommand) -> Result<Value, SocketEffectError> {
            *self.memory_dump.borrow_mut() = Some(command);
            Ok(json!({ "records": [] }))
        }
        fn log_line(&self, _line: &str) {}
        fn sidebar_state(&self, _payload: &str) {}
        fn report_git_branch(&self, _surface: SurfaceId, _branch: &str, _is_dirty: bool) {}
        fn report_pr(
            &self,
            _surface: SurfaceId,
            _number: &str,
            _label: &str,
            _status: &str,
            _branch: Option<&str>,
            _is_stale: bool,
        ) {
        }
        fn report_pwd(&self, _surface: SurfaceId, _path: &str) {}
    }

    fn parse(response: &Option<String>) -> Value {
        serde_json::from_str(response.as_ref().unwrap()).unwrap()
    }

    #[test]
    fn dispatch_v2_system_ping_returns_ok() {
        let response = dispatch(
            r#"{"id":"1","method":"system.ping"}"#,
            &FakeSocketEffects::default(),
            AuthState::UNPROTECTED,
        );
        let doc = parse(&response);
        assert_eq!(doc["ok"], json!(true));
        assert_eq!(doc["result"]["pong"], json!(true));
    }

    #[test]
    fn dispatch_unknown_v2_method_returns_method_not_found() {
        let response = dispatch(
            r#"{"id":"2","method":"unknown.method"}"#,
            &FakeSocketEffects::default(),
            AuthState::UNPROTECTED,
        );
        let doc = parse(&response);
        assert_eq!(doc["ok"], json!(false));
        assert_eq!(doc["error"]["code"], json!("method_not_found"));
    }

    #[test]
    fn notify_target_dispatches_notification_create_handler() {
        let effects = FakeSocketEffects::default();
        let response = dispatch(
            "notify_target_async ws S2 task|done|ready",
            &effects,
            AuthState::UNPROTECTED,
        );
        assert_eq!(response, Some(wire::serialize_v1_response("OK")));
        assert_eq!(effects.created_for_surface.get(), Some(SurfaceId(2)));
        assert_eq!(*effects.created_for_workspace.borrow(), "ws");
        assert_eq!(*effects.created_title.borrow(), "task");
        assert_eq!(*effects.created_subtitle.borrow(), "done");
        assert_eq!(*effects.created_body.borrow(), "ready");
    }

    #[test]
    fn create_notification_dispatches_notification_target_handler() {
        let effects = FakeSocketEffects::default();
        let response = dispatch(
            "notification.create ws S1 title|subtitle|body",
            &effects,
            AuthState::UNPROTECTED,
        );
        assert_eq!(response, Some(wire::serialize_v1_response("OK")));
        assert_eq!(effects.created_for_surface.get(), Some(SurfaceId(1)));
        assert_eq!(*effects.created_for_workspace.borrow(), "ws");
        assert_eq!(*effects.created_title.borrow(), "title");
        assert_eq!(*effects.created_subtitle.borrow(), "subtitle");
        assert_eq!(*effects.created_body.borrow(), "body");
    }

    #[test]
    fn create_for_caller_notification_dispatches_caller_handler() {
        let effects = FakeSocketEffects::default();
        let response = dispatch(
            "notification.create_for_caller title|subtitle|body",
            &effects,
            AuthState::UNPROTECTED,
        );
        assert_eq!(response, Some(wire::serialize_v1_response("OK")));
        assert_eq!(*effects.caller_title.borrow(), "title");
        assert_eq!(*effects.caller_subtitle.borrow(), "subtitle");
        assert_eq!(*effects.caller_body.borrow(), "body");
        assert!(effects.caller_preferred_surface.borrow().is_none());
    }

    #[test]
    fn mark_read_dispatches_surface_handler() {
        let effects = FakeSocketEffects::default();
        let response = dispatch(
            "notification.mark_read S2",
            &effects,
            AuthState::UNPROTECTED,
        );
        assert_eq!(response, Some(wire::serialize_v1_response("OK")));
        assert_eq!(effects.mark_read_surface.get(), Some(SurfaceId(2)));
    }

    #[test]
    fn mark_read_dispatches_all_handler() {
        let effects = FakeSocketEffects::default();
        let response = dispatch(
            "notification.mark_read all",
            &effects,
            AuthState::UNPROTECTED,
        );
        assert_eq!(response, Some(wire::serialize_v1_response("OK")));
        assert!(effects.mark_all_read.get());
    }

    #[test]
    fn dismiss_dispatches_all_read_handler() {
        let effects = FakeSocketEffects::default();
        let response = dispatch(
            "notification.dismiss all_read",
            &effects,
            AuthState::UNPROTECTED,
        );
        assert_eq!(response, Some(wire::serialize_v1_response("OK")));
        assert!(effects.dismiss_all_read_called.get());
    }

    #[test]
    fn surface_send_text_dispatches_send_handler() {
        let effects = FakeSocketEffects::default();
        let response = dispatch(
            r#"{"id":"3","method":"surface.send_text","params":{"surface_id":"S4","text":"hello"}}"#,
            &effects,
            AuthState::UNPROTECTED,
        );
        let doc = parse(&response);
        assert_eq!(doc["ok"], json!(true));
        assert_eq!(doc["result"]["ok"], json!(true));
        assert_eq!(effects.sent_surface.get(), Some(SurfaceId(4)));
        assert_eq!(*effects.sent_text.borrow(), "hello");
    }

    #[test]
    fn auth_required_blocks_command_until_authenticated() {
        let effects = FakeSocketEffects::default();
        let locked = AuthState {
            requires_authentication: true,
            is_authenticated: false,
        };

        let v1 = dispatch("send S1 hello", &effects, locked);
        assert_eq!(
            v1,
            Some(wire::serialize_v1_response("ERROR: auth required"))
        );

        let v2 = dispatch(r#"{"id":"4","method":"system.ping"}"#, &effects, locked);
        let doc = parse(&v2);
        assert_eq!(doc["ok"], json!(false));
        assert_eq!(doc["error"]["code"], json!("auth_required"));
    }

    #[test]
    fn events_stream_dispatch_returns_no_inline_response() {
        assert!(dispatch(
            "events.stream",
            &FakeSocketEffects::default(),
            AuthState::UNPROTECTED
        )
        .is_none());
        assert!(dispatch(
            r#"{"id":"5","method":"events.stream","params":{}}"#,
            &FakeSocketEffects::default(),
            AuthState::UNPROTECTED
        )
        .is_none());
    }

    #[test]
    fn surface_send_text_with_json_null_text_returns_invalid_params() {
        let effects = FakeSocketEffects::default();
        let response = dispatch(
            r#"{"id":"10","method":"surface.send_text","params":{"surface_id":"S4","text":null}}"#,
            &effects,
            AuthState::UNPROTECTED,
        );
        let doc = parse(&response);
        assert_eq!(doc["ok"], json!(false));
        assert_eq!(doc["error"]["code"], json!("invalid_params"));
        assert_eq!(*effects.sent_text.borrow(), "");
    }

    #[test]
    fn surface_send_text_with_missing_text_returns_invalid_params() {
        let response = dispatch(
            r#"{"id":"11","method":"surface.send_text","params":{"surface_id":"S4"}}"#,
            &FakeSocketEffects::default(),
            AuthState::UNPROTECTED,
        );
        let doc = parse(&response);
        assert_eq!(doc["ok"], json!(false));
        assert_eq!(doc["error"]["code"], json!("invalid_params"));
    }

    #[test]
    fn set_status_with_json_null_status_returns_invalid_params() {
        let response = dispatch(
            r#"{"id":"12","method":"set-status","params":{"status":null}}"#,
            &FakeSocketEffects::default(),
            AuthState::UNPROTECTED,
        );
        let doc = parse(&response);
        assert_eq!(doc["ok"], json!(false));
        assert_eq!(doc["error"]["code"], json!("invalid_params"));
    }

    #[test]
    fn set_status_with_invalid_surface_returns_invalid_params() {
        let response = dispatch(
            r#"{"id":"13","method":"set-status","params":{"status":"busy","surface_id":"not-a-surface"}}"#,
            &FakeSocketEffects::default(),
            AuthState::UNPROTECTED,
        );
        let doc = parse(&response);
        assert_eq!(doc["error"]["code"], json!("invalid_params"));
    }

    #[test]
    fn memory_record_dispatches_a_strict_typed_command() {
        let effects = FakeSocketEffects::default();
        let response = dispatch(
            r#"{"id":"20","method":"memory.record","params":{"surface_id":"S7","caller_capability":"cap","kind":"decision","fact":"Keep auth local","why":"Reduce exposure","file_key":"relay/auth.js","reverses":4,"defines":["RelayAuth"],"references":["Cookie"]}}"#,
            &effects,
            AuthState::UNPROTECTED,
        );
        let doc = parse(&response);
        assert_eq!(doc["ok"], json!(true));
        assert_eq!(doc["result"]["record_id"], json!(91));
        assert_eq!(
            *effects.memory_record.borrow(),
            Some(MemoryRecordCommand {
                surface: SurfaceId(7),
                caller_capability: "cap".to_string(),
                fact: "Keep auth local".to_string(),
                why: Some("Reduce exposure".to_string()),
                kind: "decision".to_string(),
                file_key: Some("relay/auth.js".to_string()),
                reverses: Some(4),
                defines: vec!["RelayAuth".to_string()],
                references: vec!["Cookie".to_string()],
            })
        );
    }

    #[test]
    fn memory_record_rejects_identity_override_fields_without_calling_effects() {
        let effects = FakeSocketEffects::default();
        for field in [
            r#""agent_id":"other""#,
            r#""workspace_id":9"#,
            r#""branch":"main""#,
            r#""trust":"trusted""#,
            r#""ts":1"#,
            r#""parent_record":2"#,
        ] {
            let frame = format!(
                r#"{{"id":"21","method":"memory.record","params":{{"surface_id":"S7","caller_capability":"cap","kind":"status","fact":"busy",{field}}}}}"#
            );
            let doc = parse(&dispatch(&frame, &effects, AuthState::UNPROTECTED));
            assert_eq!(doc["error"]["code"], json!("invalid_params"));
            assert!(effects.memory_record.borrow().is_none());
        }
    }

    #[test]
    fn memory_record_maps_caller_denial_to_a_stable_error() {
        let response = dispatch(
            r#"{"id":"22","method":"memory.record","params":{"surface_id":"S7","caller_capability":"deny","kind":"status","fact":"busy"}}"#,
            &FakeSocketEffects::default(),
            AuthState::UNPROTECTED,
        );
        let doc = parse(&response);
        assert_eq!(doc["error"]["code"], json!("caller_unauthorized"));
    }

    #[test]
    fn memory_dump_clamps_limit_and_preserves_pagination_cursor() {
        let effects = FakeSocketEffects::default();
        let response = dispatch(
            r#"{"id":"23","method":"memory.dump","params":{"surface_id":"S8","caller_capability":"cap","limit":900,"before_id":72}}"#,
            &effects,
            AuthState::UNPROTECTED,
        );
        let doc = parse(&response);
        assert_eq!(doc["ok"], json!(true));
        assert_eq!(
            *effects.memory_dump.borrow(),
            Some(MemoryDumpCommand {
                surface: SurfaceId(8),
                caller_capability: "cap".to_string(),
                limit: 500,
                before_id: Some(72),
            })
        );
    }

    #[test]
    fn report_pwd_with_json_null_path_returns_invalid_params() {
        let response = dispatch(
            r#"{"id":"13","method":"report_pwd","params":{"surface_id":"S2","path":null}}"#,
            &FakeSocketEffects::default(),
            AuthState::UNPROTECTED,
        );
        let doc = parse(&response);
        assert_eq!(doc["ok"], json!(false));
        assert_eq!(doc["error"]["code"], json!("invalid_params"));
    }

    #[test]
    fn notify_v2_with_json_null_optional_fields_falls_back_to_empty_strings() {
        let effects = FakeSocketEffects::default();
        let response = dispatch(
            r#"{"id":"14","method":"notify","params":{"surface_id":"S3","title":null,"subtitle":null,"body":null}}"#,
            &effects,
            AuthState::UNPROTECTED,
        );
        let doc = parse(&response);
        assert_eq!(doc["ok"], json!(true));
        assert_eq!(effects.created_for_surface.get(), Some(SurfaceId(3)));
        assert_eq!(*effects.created_title.borrow(), "");
        assert_eq!(*effects.created_subtitle.borrow(), "");
        assert_eq!(*effects.created_body.borrow(), "");
    }

    #[test]
    fn create_for_caller_v2_with_json_null_fields_passes_null_surface_and_empty_strings() {
        let effects = FakeSocketEffects::default();
        let response = dispatch(
            r#"{"id":"15","method":"notification.create_for_caller","params":{"preferred_surface_id":null,"title":null}}"#,
            &effects,
            AuthState::UNPROTECTED,
        );
        let doc = parse(&response);
        assert_eq!(doc["ok"], json!(true));
        assert!(effects.caller_preferred_surface.borrow().is_none());
        assert_eq!(*effects.caller_title.borrow(), "");
        assert_eq!(*effects.caller_subtitle.borrow(), "");
        assert_eq!(*effects.caller_body.borrow(), "");
    }

    #[test]
    fn auth_login_with_json_null_credential_fails_cleanly() {
        let response = dispatch(
            r#"{"id":"16","method":"auth.login","params":{"credential":null}}"#,
            &FakeSocketEffects::default(),
            AuthState::UNPROTECTED,
        );
        let doc = parse(&response);
        assert_eq!(doc["ok"], json!(true));
        assert_eq!(doc["result"]["authorized"], json!(false));
    }

    // ---- NotificationActionTests ---------------------------------------------------------------

    #[test]
    fn v2_create_routes_to_create_target_with_fields() {
        let effects = FakeSocketEffects::default();
        let response = dispatch(
            r#"{"id":"101","method":"notify","params":{"surface_id":"S3","workspace_id":"ws","title":"done","subtitle":"done-sub","body":"done-body"}}"#,
            &effects,
            AuthState::UNPROTECTED,
        );
        let doc = parse(&response);
        assert_eq!(doc["ok"], json!(true));
        assert_eq!(effects.created_for_surface.get(), Some(SurfaceId(3)));
        assert_eq!(*effects.created_for_workspace.borrow(), "ws");
        assert_eq!(*effects.created_title.borrow(), "done");
        assert_eq!(*effects.created_subtitle.borrow(), "done-sub");
        assert_eq!(*effects.created_body.borrow(), "done-body");
    }

    #[test]
    fn v2_mark_read_dispatches_all_read_marker() {
        let effects = FakeSocketEffects::default();
        let response = dispatch(
            r#"{"id":"102","method":"notification.mark_read","params":{"scope":"all"}}"#,
            &effects,
            AuthState::UNPROTECTED,
        );
        let doc = parse(&response);
        assert_eq!(doc["ok"], json!(true));
        assert!(effects.mark_all_read.get());
    }

    #[test]
    fn v2_dismiss_dispatches_all_read_marker() {
        let effects = FakeSocketEffects::default();
        let response = dispatch(
            r#"{"id":"103","method":"notification.dismiss","params":{"scope":"all_read"}}"#,
            &effects,
            AuthState::UNPROTECTED,
        );
        let doc = parse(&response);
        assert_eq!(doc["ok"], json!(true));
        assert!(effects.dismiss_all_read_called.get());
    }

    #[test]
    fn v2_create_for_caller_uses_preferred_surface() {
        let effects = FakeSocketEffects::default();
        let response = dispatch(
            r#"{"id":"104","method":"notification.create_for_caller","params":{"preferred_surface_id":"S4","title":"title","subtitle":"","body":"body"}}"#,
            &effects,
            AuthState::UNPROTECTED,
        );
        let doc = parse(&response);
        assert_eq!(doc["ok"], json!(true));
        assert_eq!(
            effects.caller_preferred_surface.borrow().as_deref(),
            Some("S4")
        );
        assert_eq!(*effects.caller_title.borrow(), "title");
        assert_eq!(*effects.caller_subtitle.borrow(), "");
        assert_eq!(*effects.caller_body.borrow(), "body");
    }
}
