//! Port of core/Ipc/SocketAccess.cs + AuthState.cs — the five-mode socket control policy and the
//! auth state threaded into the router. Pure predicates; the pipe server enforces them.

/// How the socket accepts or blocks callers. Mirrors the macOS five-mode control enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketControlMode {
    /// Socket exists but all command handling is disabled.
    Off,
    /// Default: reject unauthorized callers via ACL / peer identity only.
    OptimusOnly,
    /// Reserved for automation clients.
    Automation,
    /// Requires credential authentication before command execution.
    Password,
    /// Any same-machine caller may connect (peer-identity check disabled).
    AllowAll,
}

/// Env var holding the shared password (Password mode).
pub const PASSWORD_ENV: &str = "OPTIMUS_SOCKET_PASSWORD";
/// Env var selecting the control mode.
pub const CONTROL_MODE_ENV: &str = "OPTIMUS_SOCKET_CONTROL_MODE";

/// Parse a control-mode token, defaulting to [`SocketControlMode::OptimusOnly`] for empty/unknown.
pub fn parse_mode(value: Option<&str>) -> SocketControlMode {
    let Some(value) = value.filter(|v| !v.trim().is_empty()) else {
        return SocketControlMode::OptimusOnly;
    };
    match value.trim().to_lowercase().as_str() {
        "off" => SocketControlMode::Off,
        "optimusonly" | "optimus_only" | "optimus-only" => SocketControlMode::OptimusOnly,
        "automation" => SocketControlMode::Automation,
        "password" => SocketControlMode::Password,
        "allowall" | "allow_all" | "allow-all" => SocketControlMode::AllowAll,
        _ => SocketControlMode::OptimusOnly,
    }
}

/// Whether the mode requires password authentication before running commands.
pub fn requires_password_auth(mode: SocketControlMode) -> bool {
    mode == SocketControlMode::Password
}

/// Whether the mode enforces a peer-SID identity check (everything except `AllowAll`).
pub fn requires_peer_sid_check(mode: SocketControlMode) -> bool {
    mode != SocketControlMode::AllowAll
}

/// Whether the mode permits command execution at all (everything except `Off`).
pub fn can_run_commands(mode: SocketControlMode) -> bool {
    mode != SocketControlMode::Off
}

/// Auth state forwarded into the router so password-mode policies layer on without wiring auth
/// into dispatch itself. Port of the C# `AuthState` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthState {
    pub requires_authentication: bool,
    pub is_authenticated: bool,
}

impl AuthState {
    /// No auth required — the default when the mode is not password-protected.
    pub const UNPROTECTED: AuthState = AuthState {
        requires_authentication: false,
        is_authenticated: false,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mode_defaults_to_optimus_only() {
        assert_eq!(parse_mode(Some("")), SocketControlMode::OptimusOnly);
        assert_eq!(parse_mode(Some("unknown")), SocketControlMode::OptimusOnly);
        assert_eq!(parse_mode(None), SocketControlMode::OptimusOnly);
    }

    #[test]
    fn parse_mode_parses_all_modes() {
        assert_eq!(parse_mode(Some("off")), SocketControlMode::Off);
        assert_eq!(
            parse_mode(Some("optimus-only")),
            SocketControlMode::OptimusOnly
        );
        assert_eq!(
            parse_mode(Some("AUTOMATION")),
            SocketControlMode::Automation
        );
        assert_eq!(parse_mode(Some("password")), SocketControlMode::Password);
        assert_eq!(parse_mode(Some("allow-all")), SocketControlMode::AllowAll);
    }

    #[test]
    fn predicates_match_expected() {
        assert!(!requires_password_auth(SocketControlMode::OptimusOnly));
        assert!(requires_password_auth(SocketControlMode::Password));
        assert!(requires_peer_sid_check(SocketControlMode::OptimusOnly));
        assert!(!requires_peer_sid_check(SocketControlMode::AllowAll));
        assert!(!can_run_commands(SocketControlMode::Off));
        assert!(can_run_commands(SocketControlMode::Automation));
    }
}
