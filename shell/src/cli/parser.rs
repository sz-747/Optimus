//! Port of cli/CliParser.cs — pure argv → wire-frame parser for the `optimus` CLI (P3 unit 7).
//! No I/O: environment access goes through an injected resolver so tests drive OPTIMUS_SURFACE_ID
//! and socket/variant precedence. `Frames` are the exact newline-less V2 JSON envelopes the CLI
//! writes to the pipe, using the param names `CommandRouter` reads on the other end.
//!
//! ponytail: hand-rolled argv switch, not clap — the C# original is bespoke (positional + flag
//! mixing, join-remaining-args, env fallback); a derive parser would drift the behavior and buy
//! nothing. Add clap only if the surface grows subcommands with real option graphs.

use serde_json::{json, Map, Value};

use crate::cli::hooks;
use crate::ipc::wire::V2Request;

/// Env var the shell integration injects with the caller's surface id.
pub const SURFACE_ID_ENV: &str = "OPTIMUS_SURFACE_ID";

/// A file the invocation wants written locally (hook installer output).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFileWrite {
    pub relative_path: String,
    pub content: String,
}

/// What a parsed CLI invocation wants the process to do. Pure data (tests assert on it without a
/// pipe). Success — including usage/no-op — is `Ok`; a parse failure is [`CliError`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CliInvocation {
    pub frames: Vec<String>,
    pub explicit_socket: Option<String>,
    pub variant: Option<String>,
    pub file_writes: Option<Vec<LocalFileWrite>>,
    pub stdout: Option<String>,
    pub install_dir: Option<String>,
}

/// Parse failure with the message to print and the exit code to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    pub message: String,
    pub exit_code: i32,
}

impl CliError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            exit_code: 2,
        }
    }
}

fn usage() -> String {
    [
        "usage: optimus [--socket <pipe>] [--variant <name>] <command> [options]",
        "",
        "commands:",
        "  notify --title <t> [--subtitle <s>] [--body <b>] [--workspace <w>] [--surface <S#>]",
        "  send <surface> <text…>",
        "  send-key <surface> <key> [modifiers]",
        "  list-notifications",
        "  dismiss-notification (<id> | --surface <S#> | --all-read)",
        "  mark-notification (<id> | --surface <S#> | --all)",
        "  open-notification <id>",
        "  jump-to-unread",
        "  report_git_branch <branch> [--status dirty|clean] [--surface <S#>]",
        "  report_pr <number> [--label <l>] [--url <u>] [--pr-status open|merged|closed] [--branch <b>] [--stale] [--surface <S#>]",
        "  report_pwd <path> [--surface <S#>]",
        "  set-status <status…>",
        "  set-progress <progress…>",
        "  log <message…>",
        "  ping | capabilities",
        "  auth login --password <p>",
        "  hooks <agent> <event>            (runtime; called by installed hooks)",
        "  hooks install <agent> [--dir <d>]",
        "  hooks print <agent> [--format ps1|cmd]",
    ]
    .join("\n")
}

pub fn parse(
    args: &[&str],
    get_env: &dyn Fn(&str) -> Option<String>,
    stdin: Option<&str>,
) -> Result<CliInvocation, CliError> {
    let mut socket: Option<String> = None;
    let mut variant: Option<String> = None;
    let mut rest: Vec<&str> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let arg = args[i];
        if arg == "--socket" && i + 1 < args.len() {
            socket = Some(args[i + 1].to_string());
            i += 1;
        } else if let Some(v) = arg.strip_prefix("--socket=") {
            socket = Some(v.to_string());
        } else if arg == "--variant" && i + 1 < args.len() {
            variant = Some(args[i + 1].to_string());
            i += 1;
        } else if let Some(v) = arg.strip_prefix("--variant=") {
            variant = Some(v.to_string());
        } else {
            rest.push(arg);
        }
        i += 1;
    }

    if rest.is_empty() || matches!(rest[0], "help" | "--help" | "-h") {
        return Ok(CliInvocation {
            explicit_socket: socket,
            variant,
            stdout: Some(usage()),
            ..Default::default()
        });
    }

    let verb = rest[0];
    let tail: Vec<&str> = rest[1..].to_vec();

    let result = match verb {
        "notify" => parse_notify(&tail, get_env),
        "send" => parse_send(&tail),
        "send-key" | "send_key" => parse_send_key(&tail),
        "list-notifications" | "notification.list" => {
            Ok(single(v2("notification.list", Map::new())))
        }
        "dismiss-notification" | "notification.dismiss" => {
            parse_notification_id_verb(&tail, "notification.dismiss", "all_read", "--all-read")
        }
        "mark-notification" | "notification.mark_read" => {
            parse_notification_id_verb(&tail, "notification.mark_read", "all", "--all")
        }
        "open-notification" | "notification.open" => parse_open_notification(&tail),
        "jump-to-unread" | "notification.jump_to_unread" => {
            Ok(single(v2("notification.jump_to_unread", Map::new())))
        }
        "report_git_branch" | "report-git-branch" => parse_report_git_branch(&tail, get_env),
        "report_pr" | "report-pr" => parse_report_pr(&tail, get_env),
        "report_pwd" | "report-pwd" => parse_report_pwd(&tail, get_env),
        "set-status" | "set_status" => parse_single_string(&tail, "set-status", "status"),
        "set-progress" | "set_progress" => parse_single_string(&tail, "set-progress", "progress"),
        "log" => parse_single_string(&tail, "log", "message"),
        "ping" => Ok(single(v2("system.ping", Map::new()))),
        "capabilities" => Ok(single(v2("system.capabilities", Map::new()))),
        "auth" => parse_auth(&tail),
        "hooks" => hooks::parse(&tail, get_env, stdin),
        other => Err(CliError::new(format!(
            "unknown command \"{other}\"\n{}",
            usage()
        ))),
    };

    result.map(|inv| CliInvocation {
        explicit_socket: socket,
        variant,
        ..inv
    })
}

fn parse_notify(
    args: &[&str],
    get_env: &dyn Fn(&str) -> Option<String>,
) -> Result<CliInvocation, CliError> {
    let (mut title, mut subtitle, mut body, mut workspace, mut surface) =
        (None, None, None, None, None);
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "--title" if i + 1 < args.len() => {
                title = Some(args[i + 1]);
                i += 1;
            }
            "--subtitle" if i + 1 < args.len() => {
                subtitle = Some(args[i + 1]);
                i += 1;
            }
            "--body" if i + 1 < args.len() => {
                body = Some(args[i + 1]);
                i += 1;
            }
            "--workspace" if i + 1 < args.len() => {
                workspace = Some(args[i + 1]);
                i += 1;
            }
            "--surface" | "--window" if i + 1 < args.len() => {
                surface = Some(args[i + 1]);
                i += 1;
            }
            other => {
                return Err(CliError::new(format!(
                    "notify: unexpected argument \"{other}\""
                )))
            }
        }
        i += 1;
    }

    let title = match title {
        Some(t) if !t.is_empty() => t,
        _ => return Err(CliError::new("notify: --title is required")),
    };

    // Routing (plan U4): explicit surface → targeted notify; otherwise create_for_caller with the
    // surface id the shell integration injected into the environment (Windows has no TTY).
    if let Some(surface) = surface.filter(|s| !s.is_empty()) {
        let mut p = Map::new();
        p.insert("surface_id".into(), json!(surface));
        if let Some(ws) = workspace.filter(|w| !w.is_empty()) {
            p.insert("workspace_id".into(), json!(ws));
        }
        p.insert("title".into(), json!(title));
        p.insert("subtitle".into(), json!(subtitle.unwrap_or("")));
        p.insert("body".into(), json!(body.unwrap_or("")));
        return Ok(single(v2("notify", p)));
    }

    let caller_surface = get_env(SURFACE_ID_ENV);
    let mut p = Map::new();
    p.insert("title".into(), json!(title));
    p.insert("subtitle".into(), json!(subtitle.unwrap_or("")));
    p.insert("body".into(), json!(body.unwrap_or("")));
    if let Some(cs) = caller_surface.as_deref().filter(|s| !s.is_empty()) {
        p.insert("preferred_surface_id".into(), json!(cs));
    }
    Ok(single(v2("notification.create_for_caller", p)))
}

fn parse_send(args: &[&str]) -> Result<CliInvocation, CliError> {
    if args.len() < 2 {
        return Err(CliError::new("send: expected <surface> <text…>"));
    }
    let mut p = Map::new();
    p.insert("surface_id".into(), json!(args[0]));
    p.insert("text".into(), json!(args[1..].join(" ")));
    Ok(single(v2("surface.send_text", p)))
}

fn parse_send_key(args: &[&str]) -> Result<CliInvocation, CliError> {
    let key: u32 = match args.get(1).and_then(|s| s.parse().ok()) {
        Some(k) => k,
        None => {
            return Err(CliError::new(
                "send-key: expected <surface> <key> [modifiers]",
            ))
        }
    };
    let modifiers: u32 = match args.get(2) {
        Some(m) => match m.parse() {
            Ok(v) => v,
            Err(_) => return Err(CliError::new("send-key: invalid modifiers")),
        },
        None => 0,
    };
    let mut p = Map::new();
    p.insert("surface_id".into(), json!(args[0]));
    p.insert("key".into(), json!(key));
    p.insert("modifiers".into(), json!(modifiers));
    Ok(single(v2("surface.send_key", p)))
}

fn parse_notification_id_verb(
    args: &[&str],
    method: &str,
    all_scope: &str,
    all_flag: &str,
) -> Result<CliInvocation, CliError> {
    let (mut surface, mut id, mut all) = (None, None, false);
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--surface" && i + 1 < args.len() {
            surface = Some(args[i + 1]);
            i += 1;
        } else if args[i] == all_flag {
            all = true;
        } else if id.is_none() {
            id = Some(args[i]);
        } else {
            return Err(CliError::new(format!(
                "{method}: unexpected argument \"{}\"",
                args[i]
            )));
        }
        i += 1;
    }

    if id.is_none() && surface.is_none() && !all {
        return Err(CliError::new(format!(
            "{method}: expected <id>, --surface <S#>, or {all_flag}"
        )));
    }

    let mut p = Map::new();
    if let Some(id) = id {
        p.insert("id".into(), json!(id));
    } else if let Some(surface) = surface {
        p.insert("surface_id".into(), json!(surface));
    } else {
        p.insert("scope".into(), json!(all_scope));
    }
    Ok(single(v2(method, p)))
}

fn parse_open_notification(args: &[&str]) -> Result<CliInvocation, CliError> {
    if args.len() != 1 {
        return Err(CliError::new("open-notification: expected <id>"));
    }
    let mut p = Map::new();
    p.insert("id".into(), json!(args[0]));
    Ok(single(v2("notification.open", p)))
}

fn parse_report_git_branch(
    args: &[&str],
    get_env: &dyn Fn(&str) -> Option<String>,
) -> Result<CliInvocation, CliError> {
    let (mut branch, mut status, mut surface) = (None, None, None);
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--surface" && i + 1 < args.len() {
            surface = Some(args[i + 1]);
            i += 1;
        } else if args[i] == "--status" && i + 1 < args.len() {
            status = Some(args[i + 1].to_string());
            i += 1;
        } else if let Some(v) = args[i].strip_prefix("--status=") {
            status = Some(v.to_string());
        } else if branch.is_none() {
            branch = Some(args[i]);
        } else {
            return Err(CliError::new(format!(
                "report_git_branch: unexpected argument \"{}\"",
                args[i]
            )));
        }
        i += 1;
    }

    let branch = branch.ok_or_else(|| CliError::new("report_git_branch: expected <branch>"))?;
    let resolved = try_resolve_surface(surface, get_env).ok_or_else(|| {
        CliError::new("report_git_branch: no --surface and OPTIMUS_SURFACE_ID is not set")
    })?;
    let is_dirty = status
        .as_deref()
        .is_some_and(|s| s.eq_ignore_ascii_case("dirty"));

    let mut p = Map::new();
    p.insert("surface_id".into(), json!(resolved));
    p.insert("branch".into(), json!(branch));
    p.insert("is_dirty".into(), json!(is_dirty));
    Ok(single(v2("report_git_branch", p)))
}

fn parse_report_pr(
    args: &[&str],
    get_env: &dyn Fn(&str) -> Option<String>,
) -> Result<CliInvocation, CliError> {
    let (mut number, mut label, mut url, mut status, mut branch, mut surface) =
        (None, None, None, None, None, None);
    let mut stale = false;
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "--label" if i + 1 < args.len() => {
                label = Some(args[i + 1]);
                i += 1;
            }
            "--url" if i + 1 < args.len() => {
                url = Some(args[i + 1]);
                i += 1;
            }
            "--pr-status" if i + 1 < args.len() => {
                status = Some(args[i + 1]);
                i += 1;
            }
            "--branch" if i + 1 < args.len() => {
                branch = Some(args[i + 1]);
                i += 1;
            }
            "--surface" if i + 1 < args.len() => {
                surface = Some(args[i + 1]);
                i += 1;
            }
            "--stale" => stale = true,
            other => {
                if number.is_none() {
                    number = Some(other);
                } else {
                    return Err(CliError::new(format!(
                        "report_pr: unexpected argument \"{other}\""
                    )));
                }
            }
        }
        i += 1;
    }

    let number = number.ok_or_else(|| CliError::new("report_pr: expected <number>"))?;
    let resolved = try_resolve_surface(surface, get_env).ok_or_else(|| {
        CliError::new("report_pr: no --surface and OPTIMUS_SURFACE_ID is not set")
    })?;

    let mut p = Map::new();
    p.insert("surface_id".into(), json!(resolved));
    p.insert("number".into(), json!(number));
    p.insert("label".into(), json!(label.unwrap_or("")));
    p.insert("url".into(), json!(url.unwrap_or("")));
    p.insert("status".into(), json!(status.unwrap_or("")));
    if let Some(branch) = branch {
        p.insert("branch".into(), json!(branch));
    }
    p.insert("is_stale".into(), json!(stale));
    Ok(single(v2("report_pr", p)))
}

fn parse_report_pwd(
    args: &[&str],
    get_env: &dyn Fn(&str) -> Option<String>,
) -> Result<CliInvocation, CliError> {
    let (mut path, mut surface) = (None, None);
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--surface" && i + 1 < args.len() {
            surface = Some(args[i + 1]);
            i += 1;
        } else if path.is_none() {
            path = Some(args[i]);
        } else {
            return Err(CliError::new(format!(
                "report_pwd: unexpected argument \"{}\"",
                args[i]
            )));
        }
        i += 1;
    }

    let path = path.ok_or_else(|| CliError::new("report_pwd: expected <path>"))?;
    let resolved = try_resolve_surface(surface, get_env).ok_or_else(|| {
        CliError::new("report_pwd: no --surface and OPTIMUS_SURFACE_ID is not set")
    })?;

    let mut p = Map::new();
    p.insert("surface_id".into(), json!(resolved));
    p.insert("path".into(), json!(path));
    Ok(single(v2("report_pwd", p)))
}

fn parse_single_string(
    args: &[&str],
    method: &str,
    field: &str,
) -> Result<CliInvocation, CliError> {
    if args.is_empty() {
        return Err(CliError::new(format!("{method}: expected <{field}…>")));
    }
    let mut p = Map::new();
    p.insert(field.into(), json!(args.join(" ")));
    Ok(single(v2(method, p)))
}

fn parse_auth(args: &[&str]) -> Result<CliInvocation, CliError> {
    if args.first() == Some(&"login") {
        let mut password: Option<String> = None;
        let mut i = 1;
        while i < args.len() {
            if args[i] == "--password" && i + 1 < args.len() {
                password = Some(args[i + 1].to_string());
                i += 1;
            } else if let Some(v) = args[i].strip_prefix("--password=") {
                password = Some(v.to_string());
            }
            i += 1;
        }

        let password =
            password.ok_or_else(|| CliError::new("auth login: --password is required"))?;
        let mut p = Map::new();
        p.insert("credential".into(), json!(password));
        return Ok(single(v2("auth.login", p)));
    }

    Err(CliError::new("auth: expected `auth login --password <p>`"))
}

/// Resolve the surface id: explicit `--surface` wins, else the env var, else `None`.
pub(crate) fn try_resolve_surface(
    explicit: Option<&str>,
    get_env: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    if let Some(s) = explicit.filter(|s| !s.is_empty()) {
        return Some(s.to_string());
    }
    get_env(SURFACE_ID_ENV).filter(|s| !s.is_empty())
}

pub(crate) fn single(frame: String) -> CliInvocation {
    CliInvocation {
        frames: vec![frame],
        ..Default::default()
    }
}

/// Build one V2 request frame `{"id":"1","method":…,"params":{…}}` (no trailing newline — the pipe
/// client adds framing). Reuses [`V2Request`]'s Serialize so the envelope shape matches the router.
pub(crate) fn v2(method: &str, params: Map<String, Value>) -> String {
    let request = V2Request {
        id: "1".to_string(),
        method: method.to_string(),
        params: Value::Object(params),
    };
    serde_json::to_string(&request).expect("request serialization is infallible")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        }
    }

    fn parse_ok(args: &[&str], get_env: &dyn Fn(&str) -> Option<String>) -> CliInvocation {
        parse(args, get_env, None).expect("expected a successful parse")
    }

    fn single_frame(inv: &CliInvocation) -> Value {
        assert_eq!(inv.frames.len(), 1, "expected exactly one frame");
        serde_json::from_str(&inv.frames[0]).unwrap()
    }

    #[test]
    fn notify_without_surface_routes_to_create_for_caller_with_env_surface() {
        let inv = parse_ok(
            &["notify", "--title", "Build done", "--body", "All green"],
            &env(&[(SURFACE_ID_ENV, "S7")]),
        );
        let root = single_frame(&inv);
        assert_eq!(root["method"], "notification.create_for_caller");
        let p = &root["params"];
        assert_eq!(p["title"], "Build done");
        assert_eq!(p["body"], "All green");
        assert_eq!(p["preferred_surface_id"], "S7");
    }

    #[test]
    fn notify_with_explicit_surface_routes_to_targeted_notify() {
        let inv = parse_ok(
            &[
                "notify",
                "--title",
                "T",
                "--workspace",
                "ws1",
                "--surface",
                "S3",
            ],
            &no_env,
        );
        let root = single_frame(&inv);
        assert_eq!(root["method"], "notify");
        assert_eq!(root["params"]["surface_id"], "S3");
        assert_eq!(root["params"]["workspace_id"], "ws1");
    }

    #[test]
    fn notify_without_title_is_an_error() {
        assert!(parse(&["notify", "--body", "B"], &no_env, None).is_err());
    }

    #[test]
    fn send_builds_surface_send_text() {
        let inv = parse_ok(&["send", "S2", "echo", "hi"], &no_env);
        let root = single_frame(&inv);
        assert_eq!(root["method"], "surface.send_text");
        assert_eq!(root["params"]["surface_id"], "S2");
        assert_eq!(root["params"]["text"], "echo hi");
    }

    #[test]
    fn send_key_builds_surface_send_key_with_modifiers() {
        let inv = parse_ok(&["send-key", "S2", "13", "4"], &no_env);
        let root = single_frame(&inv);
        assert_eq!(root["method"], "surface.send_key");
        assert_eq!(root["params"]["key"], 13);
        assert_eq!(root["params"]["modifiers"], 4);
    }

    #[test]
    fn report_git_branch_uses_env_surface_and_dirty_flag() {
        let inv = parse_ok(
            &["report_git_branch", "main", "--status=dirty"],
            &env(&[(SURFACE_ID_ENV, "S4")]),
        );
        let root = single_frame(&inv);
        assert_eq!(root["method"], "report_git_branch");
        let p = &root["params"];
        assert_eq!(p["surface_id"], "S4");
        assert_eq!(p["branch"], "main");
        assert_eq!(p["is_dirty"], true);
    }

    #[test]
    fn report_git_branch_without_surface_or_env_is_an_error() {
        assert!(parse(&["report_git_branch", "main"], &no_env, None).is_err());
    }

    #[test]
    fn report_pr_carries_all_fields() {
        let inv = parse_ok(
            &[
                "report_pr",
                "42",
                "--label",
                "feat",
                "--url",
                "https://x/pr/42",
                "--pr-status",
                "open",
                "--branch",
                "feat/x",
                "--stale",
                "--surface",
                "S1",
            ],
            &no_env,
        );
        let p = single_frame(&inv)["params"].clone();
        assert_eq!(p["number"], "42");
        assert_eq!(p["label"], "feat");
        assert_eq!(p["url"], "https://x/pr/42");
        assert_eq!(p["status"], "open");
        assert_eq!(p["branch"], "feat/x");
        assert_eq!(p["is_stale"], true);
    }

    #[test]
    fn report_pwd_builds_path_payload() {
        let inv = parse_ok(&["report_pwd", r"C:\dev\x", "--surface", "S9"], &no_env);
        let root = single_frame(&inv);
        assert_eq!(root["method"], "report_pwd");
        assert_eq!(root["params"]["path"], r"C:\dev\x");
    }

    #[test]
    fn dismiss_notification_by_id_surface_and_scope() {
        // Numeric id, not GUID: the domain's NotificationId is u64 (P2). The parser echoes the id
        // token verbatim regardless; using a number keeps it consistent with the round-trip path.
        let by_id = single_frame(&parse_ok(&["dismiss-notification", "12345"], &no_env));
        assert_eq!(by_id["params"]["id"], "12345");

        let by_surface = single_frame(&parse_ok(
            &["dismiss-notification", "--surface", "S2"],
            &no_env,
        ));
        assert_eq!(by_surface["params"]["surface_id"], "S2");

        let by_scope = single_frame(&parse_ok(&["dismiss-notification", "--all-read"], &no_env));
        assert_eq!(by_scope["params"]["scope"], "all_read");
    }

    #[test]
    fn mark_notification_all_uses_scope_all() {
        let root = single_frame(&parse_ok(&["mark-notification", "--all"], &no_env));
        assert_eq!(root["method"], "notification.mark_read");
        assert_eq!(root["params"]["scope"], "all");
    }

    #[test]
    fn set_status_joins_remaining_args() {
        let root = single_frame(&parse_ok(
            &["set-status", "codex:", "running", "tests"],
            &no_env,
        ));
        assert_eq!(root["method"], "set-status");
        assert_eq!(root["params"]["status"], "codex: running tests");
    }

    #[test]
    fn global_socket_and_variant_flags_are_extracted() {
        let inv = parse_ok(
            &[
                "--socket",
                r"\\.\pipe\optimus-dev",
                "--variant",
                "dev",
                "ping",
            ],
            &no_env,
        );
        assert_eq!(
            inv.explicit_socket.as_deref(),
            Some(r"\\.\pipe\optimus-dev")
        );
        assert_eq!(inv.variant.as_deref(), Some("dev"));
        assert_eq!(single_frame(&inv)["method"], "system.ping");
    }

    #[test]
    fn no_args_prints_usage_with_no_frames() {
        let inv = parse_ok(&[], &no_env);
        assert!(inv.frames.is_empty());
        assert!(inv.stdout.unwrap().contains("usage:"));
    }

    #[test]
    fn unknown_verb_is_an_error() {
        let error = parse(&["frobnicate"], &no_env, None).unwrap_err();
        assert!(error.message.contains("unknown command"));
    }

    #[test]
    fn auth_login_builds_credential_payload() {
        let root = single_frame(&parse_ok(
            &["auth", "login", "--password", "hunter2"],
            &no_env,
        ));
        assert_eq!(root["method"], "auth.login");
        assert_eq!(root["params"]["credential"], "hunter2");
    }
}
