using System.Linq;
using System.Text.Json;
using Optimus.Core;
using Xunit;

namespace Optimus.Core.Tests;

public sealed class WireProtocolTests
{
    [Fact] // Covers R2 framing.
    public void Detects_v2_when_line_starts_with_brace()
    {
        Assert.True(SocketWireProtocol.IsV2Frame("{\"id\":\"1\",\"method\":\"ping\"}\n"));
        Assert.False(SocketWireProtocol.IsV2Frame("send hi\n"));
        Assert.False(SocketWireProtocol.IsV2Frame(string.Empty));
    }

    [Fact] // Covers R2 parse.
    public void Parses_v1_line_by_single_split()
    {
        V1Command parsed = SocketWireProtocol.ParseV1("notify target hello world");

        Assert.Equal("notify", parsed.Verb);
        Assert.Equal("target hello world", parsed.Args);
    }

    [Fact] // Covers R2 parse.
    public void Parses_v1_line_without_args()
    {
        V1Command parsed = SocketWireProtocol.ParseV1("events.stream\n");

        Assert.Equal("events.stream", parsed.Verb);
        Assert.Equal(string.Empty, parsed.Args);
    }

    [Fact] // Covers R2 framing and framing escape.
    public void Splits_crlf_and_lf_frames()
    {
        string payload = "a\nb\r\nc\n";
        string[] frames = [.. SocketWireProtocol.SplitFrames(payload)];

        Assert.Equal(new[] { "a", "b", "c" }, frames);
    }

    [Fact] // Covers R2.
    public void Parses_v2_json_payload()
    {
        string json = @"{""id"":""12"",""method"":""notify"",""params"":{""title"":""done""}}";
        V2Request req = SocketWireProtocol.ParseV2(json);

        Assert.Equal("12", req.Id);
        Assert.Equal("notify", req.Method);
        Assert.Equal("done", req.Params.GetProperty("title").GetString());
    }

    [Fact] // Covers R2.
    public void Serializes_v2_ok_with_newline()
    {
        var response = SocketWireProtocol.SerializeOk("7", JsonDocument.Parse("{\"ok\":true}").RootElement.Clone());

        Assert.Equal("{\"id\":\"7\",\"ok\":true,\"result\":{\"ok\":true},\"error\":null}\n", response);
    }
}
