//! Port of cli/HooksCommand.cs — agent-hook support (P3 unit 7 / plan U5). The runtime verb
//! installed hooks call (`optimus hooks <agent> <event>`) plus the installer/printer that emits the
//! `.ps1`/`.cmd` snippets. Pure: returns frames/file writes; the binary does the I/O. The snippet
//! text is a contract external agents run, so it must regenerate byte-identically.

use serde_json::{json, Map, Value};

use crate::cli::parser::{try_resolve_surface, v2, CliError, CliInvocation, LocalFileWrite};

/// One supported agent integration (ported from the macOS AgentHookDef model).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentDef {
    pub name: &'static str,
    pub display_name: &'static str,
    pub status_key: &'static str,
    pub aliases: &'static [&'static str],
}

impl AgentDef {
    pub fn matches(&self, candidate: &str) -> bool {
        self.name.eq_ignore_ascii_case(candidate)
            || self.aliases.iter().any(|a| a.eq_ignore_ascii_case(candidate))
    }
}

pub static AGENTS: &[AgentDef] = &[
    AgentDef { name: "claude", display_name: "Claude Code", status_key: "claude", aliases: &["hermes", "claude-code"] },
    AgentDef { name: "codex", display_name: "Codex", status_key: "codex", aliases: &[] },
    AgentDef { name: "gemini", display_name: "Gemini", status_key: "gemini", aliases: &[] },
    AgentDef { name: "cursor", display_name: "Cursor", status_key: "cursor", aliases: &[] },
    AgentDef { name: "copilot", display_name: "Copilot", status_key: "copilot", aliases: &[] },
];

pub static EVENTS: &[&str] = &[
    "session-start", "prompt-submit", "stop", "notification", "session-end", "session-finalize",
];

pub fn find(name: &str) -> Option<&'static AgentDef> {
    AGENTS.iter().find(|a| a.matches(name))
}

pub fn parse(
    args: &[&str],
    get_env: &dyn Fn(&str) -> Option<String>,
    stdin: Option<&str>,
) -> Result<CliInvocation, CliError> {
    match args.first() {
        None => Err(CliError::new(
            "hooks: expected `hooks <agent> <event>`, `hooks install <agent>`, or `hooks print <agent>`",
        )),
        Some(&"install") => parse_install(&args[1..]),
        Some(&"print") => parse_print(&args[1..]),
        Some(_) => parse_runtime(args, get_env, stdin),
    }
}

// ---- runtime: `optimus hooks <agent> <event>` -------------------------------------------------

fn parse_runtime(
    args: &[&str],
    get_env: &dyn Fn(&str) -> Option<String>,
    stdin: Option<&str>,
) -> Result<CliInvocation, CliError> {
    if args.len() < 2 {
        return Err(CliError::new("hooks: expected `hooks <agent> <event>`"));
    }

    let agent = find(args[0]).ok_or_else(|| {
        CliError::new(format!("hooks: unknown agent \"{}\" (known: {})", args[0], agent_names()))
    })?;

    let hook_event = args[1];
    if !EVENTS.iter().any(|e| e.eq_ignore_ascii_case(hook_event)) {
        return Err(CliError::new(format!(
            "hooks: unknown event \"{hook_event}\" (known: {})",
            EVENTS.join(", ")
        )));
    }

    // Hooks are gated on the surface id the shell integration injects: outside an optimus pane the
    // hook is a silent no-op (nothing sent) so agents work unchanged elsewhere.
    let surface_id = match try_resolve_surface(None, get_env) {
        Some(s) => s,
        None => return Ok(CliInvocation::default()),
    };

    Ok(CliInvocation {
        frames: build_runtime_frames(agent, &hook_event.to_lowercase(), &surface_id, stdin),
        ..Default::default()
    })
}

/// Maps a lifecycle event to wire frames. stop/notification raise a caller-targeted notification
/// (title = agent display name); the rest update agent status.
pub fn build_runtime_frames(
    agent: &AgentDef,
    hook_event: &str,
    surface_id: &str,
    stdin: Option<&str>,
) -> Vec<String> {
    match hook_event {
        "stop" | "notification" => {
            let body = extract_message(stdin).unwrap_or_else(|| {
                if hook_event == "stop" { "Agent finished" } else { "Agent needs attention" }.to_string()
            });
            let mut p = Map::new();
            p.insert("title".into(), json!(agent.display_name));
            p.insert("subtitle".into(), json!(""));
            p.insert("body".into(), json!(body));
            p.insert("preferred_surface_id".into(), json!(surface_id));
            vec![v2("notification.create_for_caller", p)]
        }
        "session-start" => vec![status_frame(agent, "start")],
        "prompt-submit" => vec![status_frame(agent, "busy")],
        "session-end" | "session-finalize" => vec![status_frame(agent, "idle")],
        _ => vec![],
    }
}

fn status_frame(agent: &AgentDef, state: &str) -> String {
    let mut p = Map::new();
    p.insert("status".into(), json!(format!("{}:{}", agent.status_key, state)));
    v2("set-status", p)
}

/// Pulls a human-readable message out of the hook's stdin JSON, if any.
pub fn extract_message(stdin: Option<&str>) -> Option<String> {
    let stdin = stdin?;
    if stdin.trim().is_empty() {
        return None;
    }
    // Hook stdin isn't required to be JSON; a parse failure falls through to the default body.
    let doc: Value = serde_json::from_str(stdin).ok()?;
    let obj = doc.as_object()?;
    for field in ["message", "body", "title"] {
        if let Some(Value::String(v)) = obj.get(field) {
            if !v.trim().is_empty() {
                return Some(v.clone());
            }
        }
    }
    None
}

fn agent_names() -> String {
    AGENTS.iter().map(|a| a.name).collect::<Vec<_>>().join(", ")
}

// ---- installer: `optimus hooks install <agent>` / `optimus hooks print <agent>` ---------------

fn parse_install(args: &[&str]) -> Result<CliInvocation, CliError> {
    let agent = args
        .first()
        .and_then(|name| find(name))
        .ok_or_else(|| match args.first() {
            Some(name) => CliError::new(format!("hooks install: unknown agent \"{name}\"")),
            None => CliError::new("hooks install: expected <agent>"),
        })?;

    let mut dir: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--dir" && i + 1 < args.len() {
            dir = Some(args[i + 1].to_string());
            i += 1;
        }
        i += 1;
    }

    let base = format!("optimus-{}-hook", agent.name);
    let writes = vec![
        LocalFileWrite { relative_path: format!("{base}.ps1"), content: snippet(agent, "ps1") },
        LocalFileWrite { relative_path: format!("{base}.cmd"), content: snippet(agent, "cmd") },
    ];

    Ok(CliInvocation {
        file_writes: Some(writes),
        stdout: Some(format!("installed {base}.ps1 and {base}.cmd")),
        install_dir: dir,
        ..Default::default()
    })
}

fn parse_print(args: &[&str]) -> Result<CliInvocation, CliError> {
    let agent = args
        .first()
        .and_then(|name| find(name))
        .ok_or_else(|| match args.first() {
            Some(name) => CliError::new(format!("hooks print: unknown agent \"{name}\"")),
            None => CliError::new("hooks print: expected <agent>"),
        })?;

    let mut format = "ps1".to_string();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--format" && i + 1 < args.len() {
            format = args[i + 1].to_string();
            i += 1;
        } else if let Some(v) = args[i].strip_prefix("--format=") {
            format = v.to_string();
        }
        i += 1;
    }

    if format != "ps1" && format != "cmd" {
        return Err(CliError::new("hooks print: --format must be ps1 or cmd"));
    }

    Ok(CliInvocation { stdout: Some(snippet(agent, &format)), ..Default::default() })
}

/// The hook snippet an agent is configured to execute. Forwards stdin (the hook payload JSON) to
/// the optimus runtime verb and never fails the agent (always exits 0). The OPTIMUS_SURFACE_ID gate
/// is duplicated here so the snippet is a no-op outside optimus even before optimus.exe runs.
pub fn snippet(agent: &AgentDef, format: &str) -> String {
    match format {
        "ps1" => [
            format!("# optimus {0} hook (generated by `optimus hooks install {0}`)", agent.name),
            "param([Parameter(Mandatory = $true)][string]$HookEvent)".to_string(),
            "if ([string]::IsNullOrEmpty($env:OPTIMUS_SURFACE_ID)) { exit 0 }".to_string(),
            format!("$input | & optimus hooks {} $HookEvent", agent.name),
            "exit 0".to_string(),
            String::new(),
        ]
        .join("\r\n"),
        "cmd" => [
            "@echo off".to_string(),
            format!("rem optimus {0} hook (generated by `optimus hooks install {0}`)", agent.name),
            "if \"%OPTIMUS_SURFACE_ID%\"==\"\" exit /b 0".to_string(),
            format!("optimus hooks {} %1", agent.name),
            "exit /b 0".to_string(),
            String::new(),
        ]
        .join("\r\n"),
        _ => unreachable!("snippet format is guarded by callers to ps1|cmd"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::parser::SURFACE_ID_ENV;
    use serde_json::Value;

    fn with_surface(id: &'static str) -> impl Fn(&str) -> Option<String> {
        move |key| (key == SURFACE_ID_ENV).then(|| id.to_string())
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    // Hooks are reached through the top-level CLI parser (which strips the `hooks` verb), matching
    // how installed hooks and the C# HooksCommandTests invoke them.
    use crate::cli::parser::parse as cli_parse;

    fn parse_ok(args: &[&str], get_env: &dyn Fn(&str) -> Option<String>, stdin: Option<&str>) -> CliInvocation {
        cli_parse(args, get_env, stdin).expect("expected a successful parse")
    }

    fn single_frame(inv: &CliInvocation) -> Value {
        assert_eq!(inv.frames.len(), 1);
        serde_json::from_str(&inv.frames[0]).unwrap()
    }

    #[test]
    fn stop_hook_fires_caller_notification_with_agent_display_name() {
        let inv = parse_ok(&["hooks", "claude", "stop"], &with_surface("S5"), None);
        let root = single_frame(&inv);
        assert_eq!(root["method"], "notification.create_for_caller");
        let p = &root["params"];
        assert_eq!(p["title"], "Claude Code");
        assert_eq!(p["preferred_surface_id"], "S5");
        assert!(!p["body"].as_str().unwrap().is_empty());
    }

    #[test]
    fn stop_hook_prefers_message_from_stdin_json() {
        let inv = parse_ok(
            &["hooks", "claude", "stop"],
            &with_surface("S5"),
            Some(r#"{"message":"Refactor finished, 3 files changed"}"#),
        );
        assert_eq!(single_frame(&inv)["params"]["body"], "Refactor finished, 3 files changed");
    }

    #[test]
    fn hook_without_surface_env_sends_nothing() {
        let inv = parse_ok(&["hooks", "claude", "stop"], &no_env, None);
        assert!(inv.frames.is_empty());
    }

    #[test]
    fn agent_aliases_resolve() {
        let inv = parse_ok(&["hooks", "hermes", "stop"], &with_surface("S1"), None);
        assert_eq!(single_frame(&inv)["params"]["title"], "Claude Code");
    }

    #[test]
    fn unknown_agent_and_unknown_event_are_errors() {
        assert!(cli_parse(&["hooks", "clippy", "stop"], &no_env, None).is_err());
        assert!(cli_parse(&["hooks", "claude", "explode"], &no_env, None).is_err());
    }

    #[test]
    fn lifecycle_events_map_to_status_updates() {
        for (hook_event, expected) in [
            ("session-start", "claude:start"),
            ("prompt-submit", "claude:busy"),
            ("session-end", "claude:idle"),
            ("session-finalize", "claude:idle"),
        ] {
            let inv = parse_ok(&["hooks", "claude", hook_event], &with_surface("S1"), None);
            let root = single_frame(&inv);
            assert_eq!(root["method"], "set-status");
            assert_eq!(root["params"]["status"], expected);
        }
    }

    #[test]
    fn install_emits_ps1_and_cmd_snippets() {
        let inv = parse_ok(&["hooks", "install", "codex", "--dir", r"C:\tmp\hooks"], &no_env, None);
        let writes = inv.file_writes.as_ref().unwrap();
        assert_eq!(writes.len(), 2);
        assert_eq!(inv.install_dir.as_deref(), Some(r"C:\tmp\hooks"));
        assert!(writes.iter().any(|w| w.relative_path == "optimus-codex-hook.ps1"));
        assert!(writes.iter().any(|w| w.relative_path == "optimus-codex-hook.cmd"));
        assert!(inv.frames.is_empty());
    }

    #[test]
    fn ps1_snippet_gates_on_surface_env_and_forwards_stdin() {
        let s = snippet(find("gemini").unwrap(), "ps1");
        assert!(s.contains("$env:OPTIMUS_SURFACE_ID"));
        assert!(s.contains("optimus hooks gemini"));
        assert!(s.contains("$input |"));
        assert!(s.contains("exit 0"));
    }

    #[test]
    fn cmd_snippet_gates_on_surface_env() {
        let s = snippet(find("cursor").unwrap(), "cmd");
        assert!(s.contains("%OPTIMUS_SURFACE_ID%"));
        assert!(s.contains("optimus hooks cursor"));
        assert!(s.contains("exit /b 0"));
    }

    #[test]
    fn print_outputs_snippet_to_stdout() {
        let inv = parse_ok(&["hooks", "print", "copilot", "--format", "cmd"], &no_env, None);
        assert!(inv.frames.is_empty());
        assert!(inv.stdout.unwrap().contains("optimus hooks copilot"));
    }

    #[test]
    fn all_planned_agents_are_covered() {
        let names: Vec<&str> = AGENTS.iter().map(|a| a.name).collect();
        assert_eq!(names, ["claude", "codex", "gemini", "cursor", "copilot"]);
    }
}
