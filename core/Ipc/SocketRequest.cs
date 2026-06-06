using System.Text.Json;
using System.Text.Json.Serialization;

namespace Cmux.Core;

/// <summary>
/// Shared request/response envelope types for socket traffic.
/// </summary>
public readonly record struct V1Command(string Verb, string Args);

/// <summary>
/// Shared V2 request envelope.
/// </summary>
public sealed record V2Request(
    [property: JsonPropertyName("id")] string Id,
    [property: JsonPropertyName("method")] string Method,
    [property: JsonPropertyName("params")] JsonElement Params)
{
    public static readonly string IdField = "id";
    public static readonly string MethodField = "method";
    public static readonly string ParamsField = "params";
}

/// <summary>
/// Shared V2 response envelope.
/// </summary>
public sealed record V2Response(
    [property: JsonPropertyName("id")] string Id,
    [property: JsonPropertyName("ok")] bool Ok,
    [property: JsonPropertyName("result")] JsonElement? Result = null,
    [property: JsonPropertyName("error")] V2Error? Error = null);

/// <summary>
/// Shared V2 error object.
/// </summary>
public sealed record V2Error(
    [property: JsonPropertyName("code")] string Code,
    [property: JsonPropertyName("message")] string Message);

/// <summary>
/// Shared V2 notification frame payload for events.stream.
/// </summary>
public sealed record V2Frame(string Method, JsonElement Params);
