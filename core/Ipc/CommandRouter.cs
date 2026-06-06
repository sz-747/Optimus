using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Text.Json;

namespace Cmux.Core;

/// <summary>
/// Pure socket dispatch for the Phase 4 IPC contract. Parses one framed line, validates the command
/// against auth state, runs the mapped effect, and returns a single framed response.
/// </summary>
public static class CommandRouter
{
    private const string ParseErrorCode = "parse_error";
    private const string MethodNotFoundCode = "method_not_found";
    private const string InvalidParamsCode = "invalid_params";
    private const string AuthRequiredCode = "auth_required";

    public static string? Dispatch(string line, ISocketEffects effects, AuthState authState)
    {
        if (effects is null)
        {
            throw new ArgumentNullException(nameof(effects));
        }

        if (string.IsNullOrWhiteSpace(line))
        {
            return SocketWireProtocol.SerializeV1Response("ERROR: empty request");
        }

        if (SocketWireProtocol.IsV2Frame(line))
        {
            try
            {
                return Dispatch(SocketWireProtocol.ParseV2(line), effects, authState);
            }
            catch (FormatException)
            {
                return SocketWireProtocol.SerializeError("0", ParseErrorCode, "Invalid V2 request.");
            }
            catch (JsonException)
            {
                return SocketWireProtocol.SerializeError("0", ParseErrorCode, "Invalid V2 request.");
            }
        }

        return Dispatch(SocketWireProtocol.ParseV1(line), effects, authState);
    }

    public static string? Dispatch(V1Command command, ISocketEffects effects, AuthState authState)
    {
        if (SocketMethods.EventsStream.Equals(command.Verb, StringComparison.Ordinal))
        {
            return null;
        }

        if (IsAuthRequired(command.Verb, authState))
        {
            return SocketWireProtocol.SerializeV1Response("ERROR: auth required");
        }

        return command.Verb switch
        {
            SocketMethods.Send => HandleSendText(command.Args, effects),
            SocketMethods.SendKey => HandleSendKey(command.Args, effects),
            SocketMethods.Notify => HandleNotifyTarget(command.Args, effects),
            SocketMethods.NotifyTarget => HandleNotifyTarget(command.Args, effects),
            SocketMethods.ListNotifications => HandleListNotifications(effects.NotificationList()),
            SocketMethods.DismissNotification => HandleNotificationDismiss(command.Args, effects),
            SocketMethods.DismissNotificationForSurface => HandleNotificationDismissForSurface(command.Args, effects),
            SocketMethods.DismissAllNotifications => HandleNotificationClear(effects),
            SocketMethods.ClearReadNotifications => HandleNotificationMarkRead(command.Args, effects),
            SocketMethods.OpenNotification => HandleNotificationOpen(command.Args, effects),
            SocketMethods.JumpToUnread => HandleJumpToUnread(effects),
            SocketMethods.SetStatus => HandleSetStatus(command.Args, effects),
            SocketMethods.SetProgress => HandleSetProgress(command.Args, effects),
            SocketMethods.LogLine => HandleLog(command.Args, effects),
            SocketMethods.SidebarState => HandleSidebarState(command.Args, effects),
            SocketMethods.Auth => HandleAuth(command.Args, effects),
            SocketMethods.ReportGitBranch => HandleReportGitBranch(command.Args, effects),
            SocketMethods.ReportPr => HandleReportPr(command.Args, effects),
            SocketMethods.ReportPwd => HandleReportPwd(command.Args, effects),
            _ => SocketWireProtocol.SerializeV1Response($"ERROR: unknown command \"{command.Verb}\""),
        };
    }

    public static string? Dispatch(V2Request request, ISocketEffects effects, AuthState authState)
    {
        if (SocketMethods.EventsStream.Equals(request.Method, StringComparison.Ordinal))
        {
            return null;
        }

        if (IsAuthRequired(request.Method, authState))
        {
            return SocketWireProtocol.SerializeError(request.Id, AuthRequiredCode, "Authentication required.");
        }

        return request.Method switch
        {
            SocketMethods.SystemPing => Ok(request, new { pong = true }),
            SocketMethods.SystemCapabilities => Ok(request, new { capabilities = effects.Capabilities() }),
            SocketMethods.V2Notify => HandleNotifyV2(request, effects),
            SocketMethods.EventsStreamV2 => null,
            SocketMethods.AuthLogin => HandleAuthLogin(request, effects),
            SocketMethods.SurfaceSendText => HandleSurfaceSendText(request, effects),
            SocketMethods.SurfaceSendKey => HandleSurfaceSendKey(request, effects),
            SocketMethods.SetStatus => HandleSetStatusV2(request, effects),
            SocketMethods.SetProgress => HandleSetProgressV2(request, effects),
            SocketMethods.LogLine => HandleLogV2(request, effects),
            SocketMethods.SidebarState => HandleSidebarStateV2(request, effects),
            SocketMethods.ListNotifications => Ok(request, new { notifications = SerializeNotifications(effects.NotificationList()) }),
            SocketMethods.DismissAllNotifications => Ok(request, HandleNotificationClear(effects)),
            SocketMethods.DismissNotification => HandleNotificationDismissV2(request, effects),
            SocketMethods.DismissNotificationForSurface => HandleNotificationDismissForSurfaceV2(request, effects),
            SocketMethods.ClearReadNotifications => HandleNotificationMarkReadV2(request, effects),
            SocketMethods.OpenNotification => HandleNotificationOpenV2(request, effects),
            SocketMethods.JumpToUnread => Ok(request, new { jumped = effects.JumpToUnread() }),
            SocketMethods.ReportGitBranch => HandleReportGitBranchV2(request, effects),
            SocketMethods.ReportPr => HandleReportPrV2(request, effects),
            SocketMethods.ReportPwd => HandleReportPwdV2(request, effects),
            SocketMethods.SurfaceFocus => HandleSurfaceFocusV2(request, effects),
            _ => ParseError(request, MethodNotFoundCode, "Unknown method."),
        };
    }

    private static string? HandleSendText(string args, ISocketEffects effects)
    {
        if (!TryParseSurfacePayload(args, out SurfaceId surface, out string payload))
        {
            return SocketWireProtocol.SerializeV1Response("ERROR: expected: send <surface> <text>");
        }

        effects.SendText(surface, payload);
        return SocketWireProtocol.SerializeV1Response("OK");
    }

    private static string? HandleSendKey(string args, ISocketEffects effects)
    {
        if (!TryParseSurfacePayload(args, out SurfaceId surface, out string payload))
        {
            return SocketWireProtocol.SerializeV1Response("ERROR: expected: send-key <surface> <key> [modifiers]");
        }

        string[] parts = payload.Split(' ', StringSplitOptions.RemoveEmptyEntries);
        if (!uint.TryParse(parts.FirstOrDefault(), NumberStyles.Integer, CultureInfo.InvariantCulture, out uint key))
        {
            return SocketWireProtocol.SerializeV1Response("ERROR: invalid key.");
        }

        uint modifiers = 0;
        if (parts.Length > 1 && !uint.TryParse(parts[1], NumberStyles.Integer, CultureInfo.InvariantCulture, out modifiers))
        {
            return SocketWireProtocol.SerializeV1Response("ERROR: invalid modifiers.");
        }

        effects.SendKey(surface, key, modifiers);
        return SocketWireProtocol.SerializeV1Response("OK");
    }

    private static string? HandleNotifyTarget(string args, ISocketEffects effects)
    {
        if (!TryParseWorkspaceSurfaceAndRemainder(args, out string? workspace, out SurfaceId surface, out string body))
        {
            return SocketWireProtocol.SerializeV1Response("ERROR: expected: notify_target_async <workspace> <surface> <title>|<subtitle>|<body>");
        }

        string[] parts = body.Split('|', 3);
        if (parts.Length < 1)
        {
            return SocketWireProtocol.SerializeV1Response("ERROR: expected: <title>|<subtitle>|<body>");
        }

        string title = parts[0];
        string subtitle = parts.Length > 1 ? parts[1] : string.Empty;
        string message = parts.Length > 2 ? parts[2] : string.Empty;

        effects.CreateNotificationForTarget(workspace ?? string.Empty, surface, title, subtitle, message);
        return SocketWireProtocol.SerializeV1Response("OK");
    }

    private static string HandleListNotifications(IReadOnlyList<TerminalNotification> notifications) =>
        SocketWireProtocol.SerializeV1Response($"OK: {notifications.Count} notifications");

    private static string? HandleNotificationDismiss(string args, ISocketEffects effects)
    {
        if (!TryParseGuid(args, out Guid id))
        {
            return SocketWireProtocol.SerializeV1Response("ERROR: expected notification id.");
        }

        effects.NotificationDismiss(id);
        return SocketWireProtocol.SerializeV1Response("OK");
    }

    private static string? HandleNotificationDismissForSurface(string args, ISocketEffects effects)
    {
        if (!TryParseSurface(args, out SurfaceId surface))
        {
            return SocketWireProtocol.SerializeV1Response("ERROR: expected notification surface id.");
        }

        effects.NotificationDismissForSurface(surface);
        return SocketWireProtocol.SerializeV1Response("OK");
    }

    private static string? HandleNotificationClear(ISocketEffects effects)
    {
        effects.NotificationClear();
        return SocketWireProtocol.SerializeV1Response("OK");
    }

    private static string? HandleNotificationMarkRead(string args, ISocketEffects effects)
    {
        if (!TryParseGuid(args, out Guid id))
        {
            return SocketWireProtocol.SerializeV1Response("ERROR: expected notification id.");
        }

        effects.NotificationMarkRead(id);
        return SocketWireProtocol.SerializeV1Response("OK");
    }

    private static string? HandleNotificationOpen(string args, ISocketEffects effects)
    {
        if (!TryParseGuid(args, out Guid id))
        {
            return SocketWireProtocol.SerializeV1Response("ERROR: expected notification id.");
        }

        effects.NotificationOpen(id);
        return SocketWireProtocol.SerializeV1Response("OK");
    }

    private static string? HandleJumpToUnread(ISocketEffects effects)
    {
        bool jumped = effects.JumpToUnread();
        return jumped ? SocketWireProtocol.SerializeV1Response("OK") : SocketWireProtocol.SerializeV1Response("ERROR: no unread notification");
    }

    private static string? HandleSetStatus(string args, ISocketEffects effects)
    {
        effects.SetStatus(args);
        return SocketWireProtocol.SerializeV1Response("OK");
    }

    private static string? HandleSetProgress(string args, ISocketEffects effects)
    {
        effects.SetProgress(args);
        return SocketWireProtocol.SerializeV1Response("OK");
    }

    private static string? HandleLog(string args, ISocketEffects effects)
    {
        effects.LogLine(args);
        return SocketWireProtocol.SerializeV1Response("OK");
    }

    private static string? HandleSidebarState(string args, ISocketEffects effects)
    {
        effects.SidebarState(args);
        return SocketWireProtocol.SerializeV1Response("OK");
    }

    private static string? HandleAuth(string args, ISocketEffects effects)
    {
        bool ok = effects.Authenticate(args);
        return SocketWireProtocol.SerializeV1Response(ok ? "OK: auth" : "ERROR: auth failed");
    }

    private static string? HandleReportGitBranch(string args, ISocketEffects effects)
    {
        if (!TryParseWorkspaceSurfaceAndRemainder(args, out _, out SurfaceId surface, out string payload))
        {
            return SocketWireProtocol.SerializeV1Response("ERROR: expected: report_git_branch <workspace> <surface> <branch>|<isDirty>");
        }

        string[] parts = payload.Split('|', 3);
        if (parts.Length < 2)
        {
            return SocketWireProtocol.SerializeV1Response("ERROR: expected: <branch>|<isDirty>");
        }

        if (!bool.TryParse(parts[1], out bool isDirty))
        {
            return SocketWireProtocol.SerializeV1Response("ERROR: expected isDirty bool.");
        }

        effects.ReportGitBranch(surface, parts[0], isDirty);
        return SocketWireProtocol.SerializeV1Response("OK");
    }

    private static string? HandleReportPr(string args, ISocketEffects effects)
    {
        if (!TryParseWorkspaceSurfaceAndRemainder(args, out _, out SurfaceId surface, out string payload))
        {
            return SocketWireProtocol.SerializeV1Response(
                "ERROR: expected: report_pr <workspace> <surface> <num>|<label>|<url>|<status>|<branch>|<isStale>");
        }

        string[] parts = payload.Split('|', 6);
        if (parts.Length < 4)
        {
            return SocketWireProtocol.SerializeV1Response(
                "ERROR: expected: <num>|<label>|<url>|<status>|<branch>|<isStale>");
        }

        string number = parts[0];
        string label = parts[1];
        string url = parts[2];
        string status = parts[3];
        string? branch = parts.Length > 4 && parts[4].Length > 0 ? parts[4] : null;
        bool isStale = parts.Length > 5 && bool.TryParse(parts[5], out bool stale) && stale;

        effects.ReportPr(surface, number, label, status, branch, isStale);
        return SocketWireProtocol.SerializeV1Response("OK");
    }

    private static string? HandleReportPwd(string args, ISocketEffects effects)
    {
        if (!TryParseWorkspaceSurfaceAndRemainder(args, out _, out SurfaceId surface, out string payload))
        {
            return SocketWireProtocol.SerializeV1Response("ERROR: expected: report_pwd <workspace> <surface> <path>");
        }

        effects.ReportPwd(surface, payload);
        return SocketWireProtocol.SerializeV1Response("OK");
    }

    private static string? HandleNotifyV2(V2Request request, ISocketEffects effects)
    {
        if (!TryGetSurfaceFromParams(request.Params, out SurfaceId surface))
        {
            return ParseError(request, InvalidParamsCode, "Missing surface_id.");
        }

        string title = TryGetStringParam(request.Params, "title", out string? t) ? t : string.Empty;
        string subtitle = TryGetStringParam(request.Params, "subtitle", out string? s) ? s : string.Empty;
        string body = TryGetStringParam(request.Params, "body", out string? b) ? b : string.Empty;
        string workspace = TryGetStringParam(request.Params, "workspace_id", out string? workspaceValue) ? workspaceValue : string.Empty;
        effects.CreateNotificationForTarget(workspace, surface, title, subtitle, body);
        return Ok(request, new { ok = true });
    }

    private static string? HandleAuthLogin(V2Request request, ISocketEffects effects)
    {
        string credential = TryGetStringParam(request.Params, "credential", out string? c) ? c
            : TryGetStringParam(request.Params, "password", out string? p) ? p
            : string.Empty;
        bool ok = effects.Authenticate(credential);
        return Ok(request, new { authorized = ok });
    }

    private static string? HandleSurfaceSendText(V2Request request, ISocketEffects effects)
    {
        if (!TryGetSurfaceFromParams(request.Params, out SurfaceId surface))
        {
            return ParseError(request, InvalidParamsCode, "Missing surface_id.");
        }

        if (!TryGetStringParam(request.Params, "text", out string? text))
        {
            return ParseError(request, InvalidParamsCode, "Missing text.");
        }

        effects.SendText(surface, text);
        return Ok(request, new { ok = true });
    }

    private static string? HandleSurfaceSendKey(V2Request request, ISocketEffects effects)
    {
        if (!TryGetSurfaceFromParams(request.Params, out SurfaceId surface))
        {
            return ParseError(request, InvalidParamsCode, "Missing surface_id.");
        }

        if (!TryGetUIntParam(request.Params, "key", out uint key))
        {
            return ParseError(request, InvalidParamsCode, "Missing key.");
        }

        uint modifiers = 0;
        if (TryGetUIntParam(request.Params, "modifiers", out uint parsedModifiers))
        {
            modifiers = parsedModifiers;
        }
        else if (TryGetUIntParam(request.Params, "modifier", out parsedModifiers))
        {
            modifiers = parsedModifiers;
        }

        effects.SendKey(surface, key, modifiers);
        return Ok(request, new { ok = true });
    }

    private static string? HandleSurfaceFocusV2(V2Request request, ISocketEffects effects)
    {
        if (!TryGetSurfaceFromParams(request.Params, out SurfaceId surface))
        {
            return ParseError(request, InvalidParamsCode, "Missing surface_id.");
        }

        effects.FocusSurface(surface);
        return Ok(request, new { ok = true });
    }

    private static string? HandleSetStatusV2(V2Request request, ISocketEffects effects)
    {
        if (!TryGetStringParam(request.Params, "status", out string? status))
        {
            return ParseError(request, InvalidParamsCode, "Missing status.");
        }

        effects.SetStatus(status);
        return Ok(request, new { ok = true });
    }

    private static string? HandleSetProgressV2(V2Request request, ISocketEffects effects)
    {
        if (!TryGetStringParam(request.Params, "progress", out string? progress))
        {
            return ParseError(request, InvalidParamsCode, "Missing progress.");
        }

        effects.SetProgress(progress);
        return Ok(request, new { ok = true });
    }

    private static string? HandleLogV2(V2Request request, ISocketEffects effects)
    {
        string message = TryGetStringParam(request.Params, "message", out string? messageValue)
            ? messageValue
            : request.Params.ValueKind != JsonValueKind.Undefined ? request.Params.GetRawText() : string.Empty;
        effects.LogLine(message);
        return Ok(request, new { ok = true });
    }

    private static string? HandleSidebarStateV2(V2Request request, ISocketEffects effects)
    {
        effects.SidebarState(request.Params.GetRawText());
        return Ok(request, new { ok = true });
    }

    private static string? HandleNotificationDismissV2(V2Request request, ISocketEffects effects)
    {
        if (!TryGetGuidParam(request.Params, out Guid id))
        {
            return ParseError(request, InvalidParamsCode, "Missing id.");
        }
        effects.NotificationDismiss(id);
        return Ok(request, new { ok = true });
    }

    private static string? HandleNotificationDismissForSurfaceV2(V2Request request, ISocketEffects effects)
    {
        if (!TryGetSurfaceFromParams(request.Params, out SurfaceId surface))
        {
            return ParseError(request, InvalidParamsCode, "Missing surface_id.");
        }

        effects.NotificationDismissForSurface(surface);
        return Ok(request, new { ok = true });
    }

    private static string? HandleNotificationMarkReadV2(V2Request request, ISocketEffects effects)
    {
        if (!TryGetGuidParam(request.Params, out Guid id))
        {
            return ParseError(request, InvalidParamsCode, "Missing id.");
        }
        effects.NotificationMarkRead(id);
        return Ok(request, new { ok = true });
    }

    private static string? HandleNotificationOpenV2(V2Request request, ISocketEffects effects)
    {
        if (!TryGetGuidParam(request.Params, out Guid id))
        {
            return ParseError(request, InvalidParamsCode, "Missing id.");
        }

        effects.NotificationOpen(id);
        return Ok(request, new { ok = true });
    }

    private static string? HandleReportGitBranchV2(V2Request request, ISocketEffects effects)
    {
        if (!TryGetSurfaceFromParams(request.Params, out SurfaceId surface))
        {
            return ParseError(request, InvalidParamsCode, "Missing surface_id.");
        }

        string branch = TryGetStringParam(request.Params, "branch", out string? b) ? b : string.Empty;
        if (!TryGetBoolParam(request.Params, "is_dirty", out bool isDirty))
        {
            return ParseError(request, InvalidParamsCode, "Missing is_dirty.");
        }

        effects.ReportGitBranch(surface, branch, isDirty);
        return Ok(request, new { ok = true });
    }

    private static string? HandleReportPrV2(V2Request request, ISocketEffects effects)
    {
        if (!TryGetSurfaceFromParams(request.Params, out SurfaceId surface))
        {
            return ParseError(request, InvalidParamsCode, "Missing surface_id.");
        }

        string number = TryGetStringParam(request.Params, "number", out string? n) ? n : string.Empty;
        string label = TryGetStringParam(request.Params, "label", out string? l) ? l : string.Empty;
        string url = TryGetStringParam(request.Params, "url", out string? u) ? u : string.Empty;
        string status = TryGetStringParam(request.Params, "status", out string? s) ? s : string.Empty;
        string? branch = TryGetStringParam(request.Params, "branch", out string? branchValue) ? branchValue : null;
        bool isStale = TryGetBoolParam(request.Params, "is_stale", out bool stale) && stale;

        effects.ReportPr(surface, number, label, status, branch, isStale);
        return Ok(request, new { ok = true });
    }

    private static string? HandleReportPwdV2(V2Request request, ISocketEffects effects)
    {
        if (!TryGetSurfaceFromParams(request.Params, out SurfaceId surface))
        {
            return ParseError(request, InvalidParamsCode, "Missing surface_id.");
        }

        if (!TryGetStringParam(request.Params, "path", out string? path))
        {
            return ParseError(request, InvalidParamsCode, "Missing path.");
        }

        effects.ReportPwd(surface, path);
        return Ok(request, new { ok = true });
    }

    private static bool IsAuthRequired(string method, AuthState authState) =>
        authState.RequiresAuthentication
            && !authState.IsAuthenticated
            && !string.Equals(method, SocketMethods.Auth, StringComparison.Ordinal)
            && !string.Equals(method, SocketMethods.AuthLogin, StringComparison.Ordinal);

    private static string? ParseError(V2Request request, string code, string message) =>
        SocketWireProtocol.SerializeError(request.Id, code, message);

    private static string Ok(V2Request request, object result)
    {
        using JsonDocument doc = JsonDocument.Parse(JsonSerializer.Serialize(result));
        return SocketWireProtocol.SerializeOk(request, doc.RootElement);
    }

    private static IReadOnlyList<object> SerializeNotifications(IReadOnlyList<TerminalNotification> notifications) =>
        notifications.Select(n => new
        {
            id = n.Id,
            surface = n.SurfaceId.ToString(),
            title = n.Title,
            subtitle = n.Subtitle,
            body = n.Body,
            is_read = n.IsRead,
            pane_flash = n.PaneFlash,
        } as object)
        .ToList();

    private static bool TryParseSurfacePayload(string args, out SurfaceId surface, out string payload) =>
        TryParseWorkspaceSurfaceAndRemainder(args, out _, out surface, out payload);

    private static bool TryParseWorkspaceSurfaceAndRemainder(
        string args,
        out string? workspace,
        out SurfaceId surface,
        out string payload)
    {
        workspace = null;
        surface = default;
        payload = string.Empty;

        string trimmed = args.Trim();
        if (string.IsNullOrEmpty(trimmed))
        {
            return false;
        }

        int firstSpace = trimmed.IndexOf(' ');
        if (firstSpace < 0)
        {
            return false;
        }

        string first = trimmed[..firstSpace];
        string afterFirst = trimmed[(firstSpace + 1)..];

        if (TryParseSurfaceId(first, out surface))
        {
            payload = afterFirst;
            return true;
        }

        int secondSpace = afterFirst.IndexOf(' ');
        if (secondSpace < 0)
        {
            return false;
        }

        string second = afterFirst[..secondSpace];
        if (TryParseSurfaceId(second, out surface))
        {
            workspace = first;
            payload = afterFirst[(secondSpace + 1)..];
            return true;
        }

        return false;
    }

    private static bool TryParseSurface(string args, out SurfaceId surface)
    {
        surface = default;
        string trimmed = args.Trim();
        if (string.IsNullOrEmpty(trimmed))
        {
            return false;
        }

        if (TryParseSurfaceId(trimmed, out surface))
        {
            return true;
        }

        int space = trimmed.IndexOf(' ');
        if (space < 0)
        {
            return false;
        }

        string second = trimmed[(space + 1)..].TrimStart();
        return TryParseSurfaceId(second, out surface);
    }

    private static bool TryParseGuid(string args, out Guid id)
    {
        string trimmed = args.Trim();
        if (Guid.TryParse(trimmed, out id))
        {
            return true;
        }

        int space = trimmed.IndexOf(' ');
        if (space < 0)
        {
            return false;
        }

        return Guid.TryParse(trimmed[(space + 1)..].TrimStart(), out id);
    }

    private static bool TryParseSurfaceId(string value, out SurfaceId id)
    {
        if (string.IsNullOrWhiteSpace(value))
        {
            id = default;
            return false;
        }

        ReadOnlySpan<char> text = value.AsSpan();
        if (text.Length > 1 && char.ToUpperInvariant(text[0]) == 'S')
        {
            if (int.TryParse(text[1..], NumberStyles.Integer, CultureInfo.InvariantCulture, out int raw))
            {
                id = new SurfaceId(raw);
                return true;
            }
        }

        if (int.TryParse(text, NumberStyles.Integer, CultureInfo.InvariantCulture, out int numeric))
        {
            id = new SurfaceId(numeric);
            return true;
        }

        id = default;
        return false;
    }

    private static bool TryGetSurfaceFromParams(JsonElement parameters, out SurfaceId surface)
    {
        if (TryGetStringParam(parameters, "surface_id", out string? surfaceIdText)
            && TryParseSurfaceId(surfaceIdText, out surface))
        {
            return true;
        }

        if (parameters.TryGetProperty("surface", out JsonElement surfaceElement)
            && TryParseSurfaceId(surfaceElement.GetString() ?? string.Empty, out surface))
        {
            return true;
        }

        surface = default;
        return false;
    }

    private static bool TryGetStringParam(JsonElement element, string name, out string? value)
    {
        value = null;
        if (element.ValueKind != JsonValueKind.Object || !element.TryGetProperty(name, out JsonElement raw))
        {
            return false;
        }
        if (raw.ValueKind == JsonValueKind.String)
        {
            value = raw.GetString();
            return true;
        }

        value = raw.ToString();
        return !string.IsNullOrEmpty(value);
    }

    private static bool TryGetBoolParam(JsonElement element, string name, out bool value)
    {
        value = false;
        if (element.ValueKind != JsonValueKind.Object || !element.TryGetProperty(name, out JsonElement raw))
        {
            return false;
        }

        if (raw.ValueKind == JsonValueKind.True)
        {
            value = true;
            return true;
        }

        if (raw.ValueKind == JsonValueKind.False)
        {
            value = false;
            return true;
        }

        if (raw.ValueKind == JsonValueKind.String
            && bool.TryParse(raw.GetString(), out value))
        {
            return true;
        }

        return false;
    }

    private static bool TryGetUIntParam(JsonElement element, string name, out uint value)
    {
        value = 0;
        if (element.ValueKind != JsonValueKind.Object || !element.TryGetProperty(name, out JsonElement raw))
        {
            return false;
        }

        if (raw.ValueKind == JsonValueKind.Number && raw.TryGetUInt32(out uint v))
        {
            value = v;
            return true;
        }

        if (raw.ValueKind == JsonValueKind.String && uint.TryParse(raw.GetString(), out v))
        {
            value = v;
            return true;
        }

        return false;
    }

    private static bool TryGetGuidParam(JsonElement element, out Guid value)
    {
        if (element.ValueKind != JsonValueKind.Object)
        {
            value = Guid.Empty;
            return false;
        }

        if (element.TryGetProperty("id", out JsonElement raw) && raw.ValueKind == JsonValueKind.String)
        {
            return Guid.TryParse(raw.GetString(), out value);
        }
        value = Guid.Empty;
        return false;
    }
}
