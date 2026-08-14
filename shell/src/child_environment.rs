//! Child-only variables that bind a terminal process tree to its Optimus surface and control pipe.

use crate::cli::parser::{CALLER_CAPABILITY_ENV, SURFACE_ID_ENV};
use crate::domain::ids::SurfaceId;
use crate::ipc::naming::SOCKET_PATH_ENV;

pub const AGENT_ID_ENV: &str = "OPTIMUS_AGENT_ID";
pub const SESSION_ID_ENV: &str = "OPTIMUS_SESSION_ID";
pub const WORKSPACE_ID_ENV: &str = "OPTIMUS_WORKSPACE_ID";
pub const BRANCH_ENV: &str = "OPTIMUS_BRANCH";
pub const WORKTREE_ENV: &str = "OPTIMUS_WORKTREE";
pub const FILE_SCOPE_ENV: &str = "OPTIMUS_FILE_SCOPE";
pub const MODEL_API_CREDENTIALS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "AZURE_OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "GROQ_API_KEY",
    "MISTRAL_API_KEY",
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
];

/// Environment overrides for one pane's shell. Descendant agent hooks inherit these values, so a
/// caller-scoped request names the pane that launched it and returns to the same app pipe.
pub fn for_surface(surface: SurfaceId, pipe_name: &str) -> Vec<(String, String)> {
    vec![
        (SURFACE_ID_ENV.to_string(), surface.to_string()),
        (SOCKET_PATH_ENV.to_string(), pipe_name.to_string()),
    ]
}

/// Environment for an orchestrated agent. The surface + pipe pair keeps existing CLI hooks
/// caller-scoped; the durable ids let later hooks and traces join the process back to its session.
pub struct AgentEnvironment<'a> {
    pub surface: SurfaceId,
    pub pipe_name: &'a str,
    pub agent_id: &'a str,
    pub session_id: &'a str,
    pub workspace_id: &'a str,
    pub branch: &'a str,
    pub worktree: &'a str,
    pub caller_capability: &'a str,
}

pub fn for_agent(context: AgentEnvironment<'_>) -> Vec<(String, String)> {
    let mut environment = for_surface(context.surface, context.pipe_name);
    environment.extend([
        (AGENT_ID_ENV.to_string(), context.agent_id.to_string()),
        (SESSION_ID_ENV.to_string(), context.session_id.to_string()),
        (
            WORKSPACE_ID_ENV.to_string(),
            context.workspace_id.to_string(),
        ),
        (BRANCH_ENV.to_string(), context.branch.to_string()),
        (WORKTREE_ENV.to_string(), context.worktree.to_string()),
        (
            CALLER_CAPABILITY_ENV.to_string(),
            context.caller_capability.to_string(),
        ),
    ]);
    environment
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_surfaces_receive_distinct_caller_identity() {
        let first = for_surface(SurfaceId(3), r"\\.\pipe\optimus-test");
        let second = for_surface(SurfaceId(4), r"\\.\pipe\optimus-test");

        assert_eq!(first[0], (SURFACE_ID_ENV.to_string(), "S3".to_string()));
        assert_eq!(second[0], (SURFACE_ID_ENV.to_string(), "S4".to_string()));
        assert_eq!(first[1], second[1]);
    }

    #[test]
    fn orchestrated_agent_receives_durable_session_and_worktree_identity() {
        let environment = for_agent(AgentEnvironment {
            surface: SurfaceId(1_000_007),
            pipe_name: r"\\.\pipe\optimus-test",
            agent_id: "agent-7",
            session_id: "session-3",
            workspace_id: "ws-2",
            branch: "feat/seven",
            worktree: r"C:\repo\.worktrees\optimus\session-3\agent-7",
            caller_capability: "capability-secret",
        });
        assert!(environment.contains(&(AGENT_ID_ENV.to_string(), "agent-7".to_string())));
        assert!(environment.contains(&(SESSION_ID_ENV.to_string(), "session-3".to_string())));
        assert!(environment.contains(&(WORKSPACE_ID_ENV.to_string(), "ws-2".to_string())));
        assert!(environment.contains(&(BRANCH_ENV.to_string(), "feat/seven".to_string())));
        assert!(environment.iter().any(|(key, _)| key == WORKTREE_ENV));
        assert!(environment.contains(&(
            CALLER_CAPABILITY_ENV.to_string(),
            "capability-secret".to_string()
        )));
    }
}
