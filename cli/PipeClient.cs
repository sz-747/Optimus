using System;
using System.Collections.Generic;
using System.IO;
using System.IO.Pipes;
using System.Text;
using Optimus.Core;

namespace Optimus.Cli;

/// <summary>
/// Thin named-pipe client for one-shot CLI requests: connect, write newline-framed lines,
/// read one response line per request frame. Connection probing backs
/// <see cref="SocketDiscovery.ResolveWithProbe"/>.
/// </summary>
public static class PipeClient
{
    public const int DefaultConnectTimeoutMs = 2_000;
    public const int ProbeTimeoutMs = 250;

    public static bool CanConnect(string pipePath)
    {
        try
        {
            using var probe = new NamedPipeClientStream(
                ".", PipeName.ToLocalName(pipePath), PipeDirection.InOut, PipeOptions.Asynchronous);
            probe.Connect(ProbeTimeoutMs);
            return true;
        }
        catch (Exception ex) when (ex is TimeoutException or IOException or UnauthorizedAccessException)
        {
            return false;
        }
    }

    /// <summary>Sends every frame and returns one response line per frame (null on EOF).</summary>
    public static IReadOnlyList<string?> Send(string pipePath, IReadOnlyList<string> frames, int connectTimeoutMs = DefaultConnectTimeoutMs)
    {
        using var pipe = new NamedPipeClientStream(
            ".", PipeName.ToLocalName(pipePath), PipeDirection.InOut, PipeOptions.Asynchronous);
        pipe.Connect(connectTimeoutMs);

        var responses = new List<string?>(frames.Count);
        using var writer = new StreamWriter(pipe, new UTF8Encoding(false), leaveOpen: true) { AutoFlush = true, NewLine = "\n" };
        using var reader = new StreamReader(pipe, new UTF8Encoding(false), detectEncodingFromByteOrderMarks: false, leaveOpen: true);

        foreach (string frame in frames)
        {
            writer.Write(frame);
            writer.Write('\n');
            responses.Add(reader.ReadLine());
        }

        return responses;
    }
}
