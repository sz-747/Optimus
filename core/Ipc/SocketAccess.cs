using System;

namespace Cmux.Core;

/// <summary>
/// Control modes that govern how the socket accepts/blocks callers.
/// Mirrors macOS' five-mode socket control enum.
/// </summary>
public enum SocketControlMode
{
    /// <summary>
    /// The socket exists but all command handling is disabled.
    /// </summary>
    Off,

    /// <summary>
    /// Default behavior: reject unauthorized callers only through ACL / peer identity.
    /// </summary>
    CmuxOnly,

    /// <summary>
    /// Reserved for automation clients.
    /// </summary>
    Automation,

    /// <summary>
    /// Requires credential authentication before command execution.
    /// </summary>
    Password,

    /// <summary>
    /// Any same-machine caller may connect (peer identity check disabled).
    /// </summary>
    AllowAll,
}

/// <summary>
/// Access mode parsing and predicate helpers.
/// </summary>
public static class SocketAccess
{
    public const string PasswordEnvironmentVariable = "CMUX_SOCKET_PASSWORD";
    public const string ControlModeEnvironmentVariable = "CMUX_SOCKET_CONTROL_MODE";

    public static SocketControlMode ParseMode(string? value)
    {
        if (string.IsNullOrWhiteSpace(value))
        {
            return SocketControlMode.CmuxOnly;
        }

        return value.Trim().ToLowerInvariant() switch
        {
            "off" => SocketControlMode.Off,
            "cmuxonly" or "cmux_only" or "cmux-only" => SocketControlMode.CmuxOnly,
            "automation" => SocketControlMode.Automation,
            "password" => SocketControlMode.Password,
            "allowall" or "allow_all" or "allow-all" => SocketControlMode.AllowAll,
            _ => SocketControlMode.CmuxOnly,
        };
    }

    public static bool RequiresPasswordAuth(SocketControlMode mode) =>
        mode is SocketControlMode.Password;

    public static bool RequiresPeerSidCheck(SocketControlMode mode) =>
        mode is not SocketControlMode.AllowAll;

    public static bool CanRunCommands(SocketControlMode mode) =>
        mode is not SocketControlMode.Off;
}
