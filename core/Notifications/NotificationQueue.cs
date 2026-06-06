using System;
using System.Collections.Generic;

namespace Cmux.Core;

/// <summary>
/// One queued, not-yet-delivered notification (plan Phase 3 U4). The <see cref="Generation"/> is the
/// queue's internal coalescing-boundary stamp at enqueue time; callers consume <see cref="Surface"/>
/// and <see cref="Payload"/>.
/// </summary>
public readonly record struct NotificationEntry(SurfaceId Surface, SurfaceNotification Payload, long Generation);

/// <summary>
/// Absorbs notification bursts and drops orphans before anything reaches the store (plan Phase 3 U4,
/// R5). A pure, synchronous, UI-free component: the only timing/threading concern (scheduling the
/// drain on the dispatcher) lives in <c>WorkspaceView</c> (KTD4). Ported in intent from the macOS
/// <c>TerminalMutationBus</c>: a coalescing key of <c>(generation, surface)</c>, a max-16 delivery
/// cap, target revalidation via a caller-supplied liveness probe, and a generation counter so a
/// clear acts as a true boundary (KTD7).
///
/// <para>The pane is deliberately <b>not</b> part of the key or the entry: a <see cref="SurfaceId"/>
/// is globally unique and never reissued (Phase 2 KTD6), so the surface alone identifies the target.
/// The pane is re-derived from the live snapshot at delivery time by the coordinator (U6).</para>
/// </summary>
public sealed class NotificationQueue
{
    /// <summary>Maximum entries delivered in a single <see cref="Drain"/> (macOS parity).</summary>
    public const int MaxPerDrain = 16;

    private readonly List<NotificationEntry> _pending = new();

    /// <summary>
    /// The current coalescing generation. Monotonic; bumped by <see cref="MarkClearBoundary"/> so
    /// that entries enqueued after a clear are never coalesced with — or dropped by — entries from
    /// before it.
    /// </summary>
    public long Generation { get; private set; }

    /// <summary>Entries waiting to be drained (test/inspection aid).</summary>
    public int PendingCount => _pending.Count;

    /// <summary>
    /// Queue a notification for <paramref name="surface"/>. With <paramref name="coalesce"/> (the
    /// default) any pending entry for the same surface <i>in the current generation</i> is replaced,
    /// so a burst collapses to the latest payload. The Phase 4 CLI path passes
    /// <c>coalesce: false</c> so scripted notifications are never merged.
    /// </summary>
    public void Enqueue(SurfaceId surface, SurfaceNotification payload, bool coalesce = true)
    {
        if (coalesce)
        {
            _pending.RemoveAll(e => e.Generation == Generation && e.Surface == surface);
        }
        _pending.Add(new NotificationEntry(surface, payload, Generation));
    }

    /// <summary>
    /// Deliver up to <see cref="MaxPerDrain"/> live entries, oldest first. Each entry is probed with
    /// <paramref name="isLive"/>: a surface that no longer exists (the probe returns false) is an
    /// orphan and is dropped without delivery or counting against the cap (KTD5). Returns whether any
    /// entries remain pending after this drain, so the caller can reschedule (KTD4).
    /// </summary>
    public bool Drain(Func<SurfaceId, bool> isLive, Action<NotificationEntry> deliver)
    {
        int delivered = 0;
        int idx = 0;
        while (idx < _pending.Count)
        {
            NotificationEntry entry = _pending[idx];
            if (!isLive(entry.Surface))
            {
                _pending.RemoveAt(idx); // orphan: drop, do not advance idx, do not count
                continue;
            }
            if (delivered >= MaxPerDrain)
            {
                break; // cap reached; this live entry and everything after it stay pending
            }
            deliver(entry);
            delivered++;
            _pending.RemoveAt(idx);
        }
        return _pending.Count > 0;
    }

    /// <summary>
    /// Advance the coalescing generation. Pair with <see cref="ClearPending"/>: bumping first moves
    /// every live entry below the new boundary, so a subsequent clear drops them while anything
    /// enqueued afterward (at the new generation) survives.
    /// </summary>
    public void MarkClearBoundary() => Generation++;

    /// <summary>
    /// Drop pending entries that match <paramref name="inScope"/> and were enqueued before the
    /// current boundary (generation strictly less than <see cref="Generation"/>). Call
    /// <see cref="MarkClearBoundary"/> first so post-clear entries are preserved.
    /// </summary>
    public void ClearPending(Func<SurfaceId, bool> inScope)
        => _pending.RemoveAll(e => e.Generation < Generation && inScope(e.Surface));
}
