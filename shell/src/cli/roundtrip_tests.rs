//! Port of tests/Cli/CliRoundTripTests.cs — the integration seam: every frame the CLI builds must
//! be accepted by the real `CommandRouter` and land in the right effect call. This is the contract
//! test that keeps the two sides of the pipe from drifting.
//!
//! Two faithful adaptations to the P2 domain: notification ids are numeric (the domain's
//! `NotificationId` is u64, not Guid), and `report_git_branch`'s bool renders lowercase (`true`,
//! Rust's Display) where the C# test read `True`.

use std::cell::RefCell;

use serde_json::Value;

use crate::cli::parser;
use crate::domain::ids::SurfaceId;
use crate::domain::notifications::{NotificationId, TerminalNotification};
use crate::ipc::access::AuthState;
use crate::ipc::router::{self, SocketEffects};

#[derive(Default)]
struct RecordingEffects {
    calls: RefCell<Vec<String>>,
}

impl RecordingEffects {
    fn push(&self, call: String) {
        self.calls.borrow_mut().push(call);
    }
    fn single_call(&self) -> String {
        let calls = self.calls.borrow();
        assert_eq!(calls.len(), 1, "expected exactly one effect call, got {calls:?}");
        calls[0].clone()
    }
}

impl SocketEffects for RecordingEffects {
    fn capabilities(&self) -> String {
        self.push("capabilities".into());
        "v1,v2".into()
    }
    fn authenticate(&self, credential: &str) -> bool {
        self.push(format!("auth:{credential}"));
        true
    }
    fn focus_surface(&self, surface: SurfaceId) {
        self.push(format!("focus:{}", surface.0));
    }
    fn send_text(&self, surface: SurfaceId, text: &str) {
        self.push(format!("send_text:{}:{text}", surface.0));
    }
    fn send_key(&self, surface: SurfaceId, virtual_key: u32, modifiers: u32) {
        self.push(format!("send_key:{}:{virtual_key}:{modifiers}", surface.0));
    }
    fn create_notification_for_target(&self, workspace_id: &str, surface: SurfaceId, title: &str, _subtitle: &str, body: &str) {
        self.push(format!("notify_target:{workspace_id}:{}:{title}:{body}", surface.0));
    }
    fn create_notification_for_caller(&self, preferred_surface_id: Option<&str>, title: &str, _subtitle: &str, body: &str) {
        self.push(format!("notify_caller:{}:{title}:{body}", preferred_surface_id.unwrap_or("")));
    }
    fn notification_list(&self) -> Vec<TerminalNotification> {
        self.push("list".into());
        Vec::new()
    }
    fn notification_dismiss(&self, id: NotificationId) {
        self.push(format!("dismiss:{}", id.0));
    }
    fn notification_dismiss_for_surface(&self, surface: SurfaceId) {
        self.push(format!("dismiss_surface:{}", surface.0));
    }
    fn notification_dismiss_all_read(&self) {
        self.push("dismiss_all_read".into());
    }
    fn notification_clear(&self) {
        self.push("clear".into());
    }
    fn notification_mark_read(&self, id: NotificationId) {
        self.push(format!("mark:{}", id.0));
    }
    fn notification_mark_read_for_surface(&self, surface: SurfaceId) {
        self.push(format!("mark_surface:{}", surface.0));
    }
    fn notification_mark_all_read(&self) {
        self.push("mark_all".into());
    }
    fn notification_open(&self, id: NotificationId) {
        self.push(format!("open:{}", id.0));
    }
    fn jump_to_unread(&self) -> bool {
        self.push("jump".into());
        true
    }
    fn set_status(&self, status: &str) {
        self.push(format!("status:{status}"));
    }
    fn set_progress(&self, progress: &str) {
        self.push(format!("progress:{progress}"));
    }
    fn log_line(&self, line: &str) {
        self.push(format!("log:{line}"));
    }
    fn sidebar_state(&self, _payload: &str) {
        self.push("sidebar".into());
    }
    fn report_git_branch(&self, surface: SurfaceId, branch: &str, is_dirty: bool) {
        self.push(format!("git:{}:{branch}:{is_dirty}", surface.0));
    }
    fn report_pr(&self, surface: SurfaceId, number: &str, _label: &str, status: &str, _branch: Option<&str>, _is_stale: bool) {
        self.push(format!("pr:{}:{number}:{status}", surface.0));
    }
    fn report_pwd(&self, surface: SurfaceId, path: &str) {
        self.push(format!("pwd:{}:{path}", surface.0));
    }
}

fn surface(id: &'static str) -> impl Fn(&str) -> Option<String> {
    move |key| (key == parser::SURFACE_ID_ENV).then(|| id.to_string())
}

fn no_env(_: &str) -> Option<String> {
    None
}

fn run(args: &[&str], get_env: &dyn Fn(&str) -> Option<String>, stdin: Option<&str>) -> (RecordingEffects, String) {
    let inv = parser::parse(args, get_env, stdin).expect("parse ok");
    assert_eq!(inv.frames.len(), 1, "expected exactly one frame");
    let effects = RecordingEffects::default();
    let response = router::dispatch(&inv.frames[0], &effects, AuthState::UNPROTECTED).expect("a response");
    (effects, response)
}

fn assert_ok(response: &str) {
    let root: Value = serde_json::from_str(response).unwrap();
    assert_eq!(root["ok"], true, "server rejected the CLI frame: {response}");
}

#[test]
fn notify_for_caller_reaches_create_for_caller_effect() {
    let (effects, response) = run(&["notify", "--title", "T", "--body", "B"], &surface("S7"), None);
    assert_ok(&response);
    assert_eq!(effects.single_call(), "notify_caller:S7:T:B");
}

#[test]
fn targeted_notify_reaches_create_for_target_effect() {
    let (effects, response) = run(
        &["notify", "--title", "T", "--body", "B", "--workspace", "ws1", "--surface", "S3"],
        &no_env,
        None,
    );
    assert_ok(&response);
    assert_eq!(effects.single_call(), "notify_target:ws1:3:T:B");
}

#[test]
fn send_reaches_send_text_effect() {
    let (effects, response) = run(&["send", "S2", "echo hi"], &no_env, None);
    assert_ok(&response);
    assert_eq!(effects.single_call(), "send_text:2:echo hi");
}

#[test]
fn send_key_reaches_send_key_effect() {
    let (effects, response) = run(&["send-key", "S2", "13", "4"], &no_env, None);
    assert_ok(&response);
    assert_eq!(effects.single_call(), "send_key:2:13:4");
}

#[test]
fn report_git_branch_reaches_git_effect() {
    let (effects, response) = run(&["report_git_branch", "main", "--status=dirty"], &surface("S4"), None);
    assert_ok(&response);
    assert_eq!(effects.single_call(), "git:4:main:true");
}

#[test]
fn report_pr_and_pwd_reach_their_effects() {
    let (pr, pr_response) = run(&["report_pr", "42", "--pr-status", "open", "--surface", "S1"], &no_env, None);
    assert_ok(&pr_response);
    assert_eq!(pr.single_call(), "pr:1:42:open");

    let (pwd, pwd_response) = run(&["report_pwd", r"C:\dev\x", "--surface", "S9"], &no_env, None);
    assert_ok(&pwd_response);
    assert_eq!(pwd.single_call(), r"pwd:9:C:\dev\x");
}

#[test]
fn notification_actions_reach_their_effects() {
    let id = "12345";

    let (dismiss, r1) = run(&["dismiss-notification", id], &no_env, None);
    assert_ok(&r1);
    assert_eq!(dismiss.single_call(), format!("dismiss:{id}"));

    let (mark, r2) = run(&["mark-notification", "--all"], &no_env, None);
    assert_ok(&r2);
    assert_eq!(mark.single_call(), "mark_all");

    let (open, r3) = run(&["open-notification", id], &no_env, None);
    assert_ok(&r3);
    assert_eq!(open.single_call(), format!("open:{id}"));

    let (jump, r4) = run(&["jump-to-unread"], &no_env, None);
    assert_ok(&r4);
    assert_eq!(jump.single_call(), "jump");
}

#[test]
fn status_progress_and_log_reach_their_effects() {
    let (status, r1) = run(&["set-status", "busy"], &no_env, None);
    assert_ok(&r1);
    assert_eq!(status.single_call(), "status:busy");

    let (progress, r2) = run(&["set-progress", "3/5"], &no_env, None);
    assert_ok(&r2);
    assert_eq!(progress.single_call(), "progress:3/5");

    let (log, r3) = run(&["log", "hello world"], &no_env, None);
    assert_ok(&r3);
    assert_eq!(log.single_call(), "log:hello world");
}

#[test]
fn claude_stop_hook_lands_a_caller_notification() {
    let (effects, response) = run(
        &["hooks", "claude", "stop"],
        &surface("S5"),
        Some(r#"{"message":"Done refactoring"}"#),
    );
    assert_ok(&response);
    assert_eq!(effects.single_call(), "notify_caller:S5:Claude Code:Done refactoring");
}

#[test]
fn auth_login_reaches_authenticate_even_when_auth_required() {
    let inv = parser::parse(&["auth", "login", "--password", "pw"], &no_env, None).expect("parse ok");
    assert_eq!(inv.frames.len(), 1);

    let effects = RecordingEffects::default();
    let locked = AuthState { requires_authentication: true, is_authenticated: false };
    let response = router::dispatch(&inv.frames[0], &effects, locked).expect("a response");

    assert_ok(&response);
    assert_eq!(effects.single_call(), "auth:pw");
}
