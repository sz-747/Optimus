using Cmux.Core;
using Xunit;

namespace Cmux.Core.Tests;

public sealed class SocketAccessTests
{
    [Fact] // Covers R3 mode parsing.
    public void ParseMode_defaults_to_cmux_only()
    {
        Assert.Equal(SocketControlMode.CmuxOnly, SocketAccess.ParseMode(string.Empty));
        Assert.Equal(SocketControlMode.CmuxOnly, SocketAccess.ParseMode("unknown"));
    }

    [Fact] // Covers R3.
    public void ParseMode_parses_all_modes()
    {
        Assert.Equal(SocketControlMode.Off, SocketAccess.ParseMode("off"));
        Assert.Equal(SocketControlMode.CmuxOnly, SocketAccess.ParseMode("cmux-only"));
        Assert.Equal(SocketControlMode.Automation, SocketAccess.ParseMode("AUTOMATION"));
        Assert.Equal(SocketControlMode.Password, SocketAccess.ParseMode("password"));
        Assert.Equal(SocketControlMode.AllowAll, SocketAccess.ParseMode("allow-all"));
    }

    [Fact] // Covers R3.
    public void Predicates_match_expected()
    {
        Assert.False(SocketAccess.RequiresPasswordAuth(SocketControlMode.CmuxOnly));
        Assert.True(SocketAccess.RequiresPasswordAuth(SocketControlMode.Password));
        Assert.True(SocketAccess.RequiresPeerSidCheck(SocketControlMode.CmuxOnly));
        Assert.False(SocketAccess.RequiresPeerSidCheck(SocketControlMode.AllowAll));
        Assert.False(SocketAccess.CanRunCommands(SocketControlMode.Off));
        Assert.True(SocketAccess.CanRunCommands(SocketControlMode.Automation));
    }
}
