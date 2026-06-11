using System;
using System.Diagnostics;
using System.IO;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

namespace Optimus.Cli;

/// <summary>
/// Reads redirected stdin without hanging when the handle is open but silent.
/// A blocking <c>Console.In.ReadToEnd()</c> deadlocks the CLI whenever a parent
/// process redirects stdin but never writes or closes it (the pre-fix workaround
/// was piping <c>&lt; NUL</c>). Hook payloads are small one-shot JSON blobs, so the
/// policy here is: no first byte within <paramref name="firstByteTimeout"/> means
/// "no stdin" (<c>null</c>); once data starts, read until EOF, until the stream
/// goes quiet for <see cref="QuietWindow"/>, or until <paramref name="drainTimeout"/>
/// elapses — whichever comes first — and return whatever arrived.
/// </summary>
public static class StdinReader
{
    /// <summary>Default wait for the first byte before treating stdin as absent.</summary>
    public static readonly TimeSpan DefaultFirstByteTimeout = TimeSpan.FromMilliseconds(500);

    /// <summary>Default hard cap on draining after the first byte arrives.</summary>
    public static readonly TimeSpan DefaultDrainTimeout = TimeSpan.FromSeconds(2);

    /// <summary>
    /// Inactivity window that ends the drain early. Needed because some parents are
    /// not byte-silent even when they send nothing: PowerShell 5.1's Process.Start
    /// pushes a lone BOM preamble onto the redirected stdin pipe, which would
    /// otherwise make every hang-case invocation pay the full drain timeout.
    /// </summary>
    public static readonly TimeSpan QuietWindow = TimeSpan.FromMilliseconds(150);

    public static string? ReadAvailable(TextReader reader, TimeSpan firstByteTimeout, TimeSpan drainTimeout)
    {
        var buffer = new StringBuilder();
        // Deliberately not disposed: the reader task can outlive this method (a
        // blocked Read may complete much later and still call Set), so disposing
        // here would race it into ObjectDisposedException. One leaked event pair
        // in a process that exits right after is the cheaper trade.
        var firstData = new ManualResetEventSlim();
        var finished = new ManualResetEventSlim();

        Task.Run(() =>
        {
            try
            {
                char[] chunk = new char[4096];
                int read;
                while ((read = reader.Read(chunk, 0, chunk.Length)) > 0)
                {
                    lock (buffer)
                    {
                        buffer.Append(chunk, 0, read);
                    }

                    firstData.Set();
                }
            }
            catch (Exception ex) when (ex is IOException or ObjectDisposedException)
            {
                // A broken or closed pipe ends the stream; report what was read so far.
            }

            finished.Set();
        });

        int signaled = WaitHandle.WaitAny(
            new[] { firstData.WaitHandle, finished.WaitHandle },
            firstByteTimeout);
        if (signaled == WaitHandle.WaitTimeout)
        {
            return null;
        }

        var drain = Stopwatch.StartNew();
        while (!finished.IsSet && drain.Elapsed < drainTimeout)
        {
            int lengthBefore;
            lock (buffer)
            {
                lengthBefore = buffer.Length;
            }

            if (finished.Wait(QuietWindow))
            {
                break;
            }

            lock (buffer)
            {
                if (buffer.Length == lengthBefore)
                {
                    break;
                }
            }
        }

        lock (buffer)
        {
            // Strip a leading BOM so JSON payloads from BOM-emitting writers
            // (PowerShell redirectors, .NET Framework parents) still parse.
            return buffer.ToString().TrimStart('\uFEFF');
        }
    }
}
