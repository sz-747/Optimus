using System;
using System.IO;
using System.Threading;
using Microsoft.UI.Xaml;
using Cmux.Ipc;

namespace Cmux;

/// <summary>
/// Application entry point for the cmux WinUI 3 shell (plan §4). Phase 1 opens a single
/// window hosting one terminal pane.
/// </summary>
public partial class App : Application
{
    private Window? _window;
    private PipeServer? _pipeServer;
    private SocketControlMode _socketControlMode = SocketControlMode.CmuxOnly;

    public App()
    {
        this.InitializeComponent();

        // Capture any unhandled exception to crash.log so a WinUI STATUS_STOWED_EXCEPTION
        // (0xc000027b) leaves a readable trace next to the exe instead of a bare crash code.
        // We log but do not swallow (no e.Handled = true) — failures stay visible.
        this.UnhandledException += (s, e) =>
            CrashLog("Application.UnhandledException", e.Exception);
        AppDomain.CurrentDomain.UnhandledException += (s, e) =>
            CrashLog("AppDomain.UnhandledException", e.ExceptionObject as Exception);
        System.Threading.Tasks.TaskScheduler.UnobservedTaskException += (s, e) =>
            CrashLog("UnobservedTaskException", e.Exception);
    }

    private static void CrashLog(string source, Exception? ex)
    {
        try
        {
            string path = Path.Combine(AppContext.BaseDirectory, "crash.log");
            File.AppendAllText(path, $"=== {source} @ {DateTime.Now:O} ===\n{ex}\n\n");
        }
        catch
        {
            // best-effort
        }
    }

    /// <summary>
    /// Append a <b>recovered</b> (non-fatal) error to crash.log — same sink as the
    /// unhandled-exception logger. Used by UI code that catches an engine error and keeps the app
    /// running instead of letting it surface as a fatal STATUS_STOWED_EXCEPTION.
    /// </summary>
    internal static void LogError(string source, Exception? ex) => CrashLog("(recovered) " + source, ex);

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        _window = new MainWindow();
        _window.Activate();

        StartPipeServer();
    }

    private void StartPipeServer()
    {
        if (_pipeServer is not null)
        {
            return;
        }

        try
        {
            _socketControlMode = SocketAccess.ParseMode(Environment.GetEnvironmentVariable(SocketAccess.ControlModeEnvironmentVariable));
            string currentSid = PeerIdentity.GetCurrentUserSid() ?? string.Empty;
            _pipeServer = new PipeServer(
                HandleSocketLineAsync,
                controlMode: _socketControlMode,
                clientSidValidator: sid => string.Equals(sid, currentSid, StringComparison.Ordinal));
            _pipeServer.Start();
        }
        catch (Exception ex)
        {
            App.LogError("App.StartPipeServer", ex);
        }
    }

    internal void StopPipeServer()
    {
        _pipeServer?.Stop();
    }

    private static Task<string?> HandleSocketLineAsync(string request, CancellationToken cancellationToken)
    {
        _ = request;
        _ = cancellationToken;

        return Task.FromResult<string?>(SocketWireProtocol.SerializeV1Response("ERROR: socket router not wired yet"));
    }
}
