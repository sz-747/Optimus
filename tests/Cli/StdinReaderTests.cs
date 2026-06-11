using System;
using System.Diagnostics;
using System.IO;
using System.Threading;
using Optimus.Cli;
using Xunit;

namespace Optimus.Core.Tests;

/// <summary>
/// res U2: the CLI must not hang when stdin is redirected but the writer never
/// writes or closes the handle (previous workaround was invoking with &lt; NUL).
/// </summary>
public sealed class StdinReaderTests
{
    private static readonly TimeSpan Short = TimeSpan.FromMilliseconds(150);
    private static readonly TimeSpan Long = TimeSpan.FromSeconds(5);

    [Fact]
    public void Returns_full_payload_when_stdin_closes_normally()
    {
        using var reader = new StringReader("{\"message\":\"done\"}");

        string? result = StdinReader.ReadAvailable(reader, Long, Long);

        Assert.Equal("{\"message\":\"done\"}", result);
    }

    [Fact]
    public void Returns_empty_string_immediately_for_empty_closed_stdin()
    {
        using var reader = new StringReader(string.Empty);
        var sw = Stopwatch.StartNew();

        string? result = StdinReader.ReadAvailable(reader, Long, Long);

        Assert.Equal(string.Empty, result);
        // EOF must short-circuit the first-byte wait, not sit out the full timeout.
        Assert.True(sw.Elapsed < TimeSpan.FromSeconds(2), $"took {sw.Elapsed}");
    }

    [Fact]
    public void Returns_null_when_stdin_stays_open_and_silent()
    {
        using var reader = new BlockingReader(initial: null);

        string? result = StdinReader.ReadAvailable(reader, Short, Short);

        Assert.Null(result);
    }

    [Fact]
    public void Returns_partial_payload_when_writer_never_closes_after_writing()
    {
        using var reader = new BlockingReader(initial: "{\"message\":\"hi\"}");

        string? result = StdinReader.ReadAvailable(reader, Long, Short);

        Assert.Equal("{\"message\":\"hi\"}", result);
    }

    [Fact]
    public void Quiet_window_ends_drain_early_instead_of_waiting_full_drain_timeout()
    {
        // PowerShell 5.1 parents push a lone BOM onto the pipe, then go silent;
        // the drain must end at the quiet window, not sit out the hard cap.
        using var reader = new BlockingReader(initial: "﻿");
        var sw = Stopwatch.StartNew();

        string? result = StdinReader.ReadAvailable(reader, Long, drainTimeout: TimeSpan.FromSeconds(10));

        Assert.Equal(string.Empty, result);
        Assert.True(sw.Elapsed < TimeSpan.FromSeconds(5), $"took {sw.Elapsed}");
    }

    [Fact]
    public void Strips_leading_bom_from_payload()
    {
        using var reader = new StringReader("﻿{\"message\":\"done\"}");

        string? result = StdinReader.ReadAvailable(reader, Long, Long);

        Assert.Equal("{\"message\":\"done\"}", result);
    }

    /// <summary>Serves an optional initial payload, then blocks forever — an open, silent pipe.</summary>
    private sealed class BlockingReader : TextReader
    {
        private readonly SemaphoreSlim _block = new(0);
        private string? _pending;

        public BlockingReader(string? initial) => _pending = initial;

        public override int Read(char[] buffer, int index, int count)
        {
            if (_pending is { Length: > 0 } pending)
            {
                int n = Math.Min(count, pending.Length);
                pending.CopyTo(0, buffer, index, n);
                _pending = pending[n..];
                return n;
            }

            _block.Wait();
            return 0;
        }

        protected override void Dispose(bool disposing)
        {
            if (disposing)
            {
                _block.Release(short.MaxValue);
            }

            base.Dispose(disposing);
        }
    }
}
