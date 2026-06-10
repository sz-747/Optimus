using Optimus.Core;
using Xunit;

namespace Optimus.Core.Tests;

public sealed class SocketAccessTests
{
    [Fact] // Covers R3 mode parsing.
    public void ParseMode_defaults_to_optimus_only()
    {
        Assert.Equal(SocketControlMode.OptimusOnly, SocketAccess.ParseMode(string.Empty));
        Assert.Equal(SocketControlMode.OptimusOnly, SocketAccess.ParseMode("unknown"));
    }

    [Fact] // Covers R3.
    public void ParseMode_parses_all_modes()
    {
        Assert.Equal(SocketControlMode.Off, SocketAccess.ParseMode("off"));
        Assert.Equal(SocketControlMode.OptimusOnly, SocketAccess.ParseMode("optimus-only"));
        Assert.Equal(SocketControlMode.Automation, SocketAccess.ParseMode("AUTOMATION"));
        Assert.Equal(SocketControlMode.Password, SocketAccess.ParseMode("password"));
        Assert.Equal(SocketControlMode.AllowAll, SocketAccess.ParseMode("allow-all"));
    }

    [Fact] // Covers R3.
    public void Predicates_match_expected()
    {
        Assert.False(SocketAccess.RequiresPasswordAuth(SocketControlMode.OptimusOnly));
        Assert.True(SocketAccess.RequiresPasswordAuth(SocketControlMode.Password));
        Assert.True(SocketAccess.RequiresPeerSidCheck(SocketControlMode.OptimusOnly));
        Assert.False(SocketAccess.RequiresPeerSidCheck(SocketControlMode.AllowAll));
        Assert.False(SocketAccess.CanRunCommands(SocketControlMode.Off));
        Assert.True(SocketAccess.CanRunCommands(SocketControlMode.Automation));
    }
}
