using System;
using System.Collections.Generic;
using System.Text.Json;

namespace Cmux.Core;

/// <summary>
/// Newline-framed wire helpers shared by CLI and app socket handlers.
/// </summary>
public static class SocketWireProtocol
{
    private const char V2Sentinel = '{';

    public static bool IsV2Frame(string line)
    {
        if (string.IsNullOrEmpty(line))
        {
            return false;
        }

        return TrimTrailingNewline(line).Length > 0 && TrimTrailingNewline(line)[0] == V2Sentinel;
    }

    public static V1Command ParseV1(string line)
    {
        string clean = TrimTrailingNewline(line);
        int firstSpace = clean.IndexOf(' ');
        if (firstSpace < 0)
        {
            return new V1Command(clean, string.Empty);
        }

        string verb = clean[..firstSpace];
        string args = clean[(firstSpace + 1)..];
        return new V1Command(verb, args);
    }

    public static V2Request ParseV2(string line)
    {
        string clean = TrimTrailingNewline(line);
        JsonDocument doc = JsonDocument.Parse(clean);
        JsonElement root = doc.RootElement;

        if (root.ValueKind != JsonValueKind.Object ||
            !root.TryGetProperty(V2Request.IdField, out JsonElement idEl) ||
            idEl.ValueKind != JsonValueKind.String ||
            !root.TryGetProperty(V2Request.MethodField, out JsonElement methodEl) ||
            methodEl.ValueKind != JsonValueKind.String)
        {
            throw new FormatException("Invalid V2 request.");
        }

        JsonElement paramsEl = root.TryGetProperty(V2Request.ParamsField, out JsonElement p)
            ? p
            : JsonDocument.Parse("{}").RootElement;

        return new V2Request(idEl.GetString()!, methodEl.GetString()!, paramsEl.Clone());
    }

    public static string SerializeOk(V2Request request, JsonElement result)
    {
        var response = new V2Response(request.Id, Ok: true, result, Error: null);
        return JsonSerializer.Serialize(response) + '\n';
    }

    public static string SerializeOk(string requestId, JsonElement result) =>
        JsonSerializer.Serialize(new V2Response(requestId, Ok: true, result, null)) + '\n';

    public static string SerializeError(string requestId, string code, string message) =>
        JsonSerializer.Serialize(
            new V2Response(
                requestId,
                Ok: false,
                Result: null,
                Error: new V2Error(code, message))) + '\n';

    public static string SerializeV1Response(string payload) => payload + '\n';

    public static IEnumerable<string> SplitFrames(string buffer)
    {
        int start = 0;
        for (int i = 0; i <= buffer.Length; i++)
        {
            if (i < buffer.Length && buffer[i] != '\n')
            {
                continue;
            }

            if (i - start <= 0)
            {
                start = i + 1;
                continue;
            }

            string frame = buffer[start..i];
            if (frame.Length > 0 && frame[^1] == '\r')
            {
                frame = frame[..^1];
            }

            yield return frame;
            start = i + 1;
        }
    }

    private static string TrimTrailingNewline(string text)
    {
        if (string.IsNullOrEmpty(text))
        {
            return string.Empty;
        }

        if (text.EndsWith('\n'))
        {
            return text.EndsWith("\r\n") ? text[..^2] : text[..^1];
        }

        return text;
    }
}
