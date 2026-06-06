using System;
using System.Collections.Generic;
using Cmux.Core;
using Xunit;

namespace Cmux.Core.Tests;

public sealed class SocketDiscoveryTests
{
    [Fact] // Covers R4.
    public void Resolve_path_prefers_socket_path_over_socket()
    {
        string? Resolver(string key) => key switch
        {
            PipeName.SocketPathEnv => @"\\.\pipe\from-path",
            PipeName.SocketEnv => @"\\.\pipe\from-legacy",
            _ => null
        };

        string path = PipeName.ResolveFromEnvironment(Resolver);

        Assert.Equal(@"\\.\pipe\from-path", path);
    }

    [Fact] // Covers R4.
    public void BuildPipeName_uses_stable_by_default()
    {
        Assert.Equal(@"\\.\pipe\cmux-stable", PipeName.BuildPipeName(string.Empty));
    }

    [Fact] // Covers R4.
    public void BuildCandidatePipes_orders_preferred_first()
    {
        IReadOnlyList<string> list = SocketDiscovery.BuildCandidatePipes(
            null,
            explicitVariant: "nightly",
            slug: "beta");

        Assert.Equal(@"\\.\pipe\cmux-nightly-beta", list[0]);
        Assert.Equal(@"\\.\pipe\cmux-stable-beta", list[1]);
    }

    [Fact] // Covers R4.
    public void ResolveWithProbe_selects_first_connectable()
    {
        IReadOnlyList<string> candidates = [
            @"\\.\pipe\down",
            @"\\.\pipe\up",
            @"\\.\pipe\later"
        ];
        string chosen = SocketDiscovery.ResolveWithProbe(candidates, pipe => pipe.Contains("up"));

        Assert.Equal(@"\\.\pipe\up", chosen);
    }

    [Fact]
    public void ResolveWithProbe_falls_back_to_preferred_when_none_connect()
    {
        IReadOnlyList<string> candidates = ["a", "b"];
        string chosen = SocketDiscovery.ResolveWithProbe(candidates, _ => false);

        Assert.Equal("a", chosen);
    }
}
