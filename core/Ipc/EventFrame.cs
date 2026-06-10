using System.Text.Json;

namespace Optimus.Core;

/// <summary>
/// Shared event frame shapes for events.stream.
/// </summary>
public abstract record EventFrame(string Event);

public sealed record EventAckFrame(long Seq, string? EventId = null) : EventFrame(Event: "ack");

public sealed record EventPayloadFrame(long Seq, string Name, JsonElement Data) : EventFrame(Event: "event");

public sealed record EventHeartbeatFrame(long Seq, long TimestampMillis) : EventFrame(Event: "heartbeat");
