namespace Cmux.Core;

/// <summary>
/// The raw notification payload as it leaves a surface (Phase 3 U2): the title and body the engine
/// extracted from an OSC 9 / 777 / 99 escape sequence, marshalled to the UI thread and copied into
/// owned strings by <c>EngineHandle</c> before it reaches here. Deliberately UI-free and minimal —
/// the recorded model (<see cref="TerminalNotification"/>), ids, timestamps, and read state are
/// added by the store (U3), not carried on the wire. Lives in <c>Cmux.Core</c> so the event contract
/// is testable and fakes can raise it without a window.
/// </summary>
public readonly record struct SurfaceNotification(string Title, string Subtitle, string Body);
