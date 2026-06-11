using System;
using System.IO;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.UI.Xaml;
using Optimus.Capacity;
using Optimus.Ipc;
using Optimus.Core;

namespace Optimus;

/// <summary>
/// Application entry point for the optimus WinUI 3 shell (plan §4). Phase 1 opens a single
/// window hosting one terminal pane.
/// </summary>
public partial class App : Application
{
    private Window? _window;
    private PipeServer? _pipeServer;
    private SocketControlMode _socketControlMode = SocketControlMode.OptimusOnly;
    private PipeServerEffects? _pipeServerEffects;
    private readonly PasswordStore _passwordStore = new();
    private bool _socketAuthenticated;
    private Win32CapacityProvider? _capacityProvider;
    private CapacityTicker? _capacityTicker;

    /// <summary>
    /// App-wide RAM safe-zone governor (plan U3). Composed in <see cref="OnLaunched"/> before the
    /// main window so the spawn gate (U5) and the sidebar indicator (U6) can reach it. Null only
    /// if composition failed (logged, non-fatal) — consumers must tolerate that.
    /// </summary>
    internal static CapacityModel? Capacity { get; private set; }

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
        StartCapacityGovernor();

        _window = new MainWindow();
        _window.Activate();

        StartPipeServer();
    }

    /// <summary>
    /// Compose the RAM safe-zone governor: Win32 provider + persisted calibration + 1 Hz ticker
    /// (plan U3). Failure is logged and leaves <see cref="Capacity"/> null — the app must still
    /// launch as a plain multiplexer rather than crash over a measurement problem.
    /// </summary>
    private void StartCapacityGovernor()
    {
        if (Capacity is not null)
        {
            return;
        }

        try
        {
            _capacityProvider = new Win32CapacityProvider();
            var model = new CapacityModel(_capacityProvider, new JsonCalibrationStore());
            model.LoadCalibration();
            model.StateChanged += state =>
                System.Diagnostics.Debug.WriteLine(
                    $"[capacity] used={state.Used} reserved={state.Reserved} max={state.Max} level={state.Level}");
            _capacityTicker = new CapacityTicker(model, _capacityProvider);
            Capacity = model;
        }
        catch (Exception ex)
        {
            LogError("App.StartCapacityGovernor", ex);
            // Order matters: dispose the ticker FIRST (it unregisters + drains the thread-pool
            // wait on the provider's notification handle), THEN the provider (which closes that
            // handle). The reverse order would let an in-flight callback touch a closed handle.
            _capacityTicker?.Dispose();
            _capacityTicker = null;
            _capacityProvider?.Dispose();
            _capacityProvider = null;
        }
    }

    /// <summary>
    /// Tear down the governor on graceful shutdown — the matching end of
    /// <see cref="StartCapacityGovernor"/>, driven by the main window's <c>Closed</c> handler.
    /// Persists the learned per-terminal budget so the next session starts already-calibrated
    /// instead of re-learning from the seed. Idempotent; tolerates a governor that never started.
    /// </summary>
    internal void StopCapacityGovernor()
    {
        // Same order as the StartCapacityGovernor failure path: ticker first (unregisters and
        // drains the thread-pool wait on the provider's notification handle, bounded 2s+2s — a
        // pathologically stuck callback could still tick once after this returns; the budget it
        // writes is lock-protected and simply misses this save), then the provider.
        // SaveCalibration runs before the provider it reads TotalPhysBytes from is disposed, and
        // Capacity is unpublished before that disposal so no consumer can reach the model while
        // its provider is being torn down.
        _capacityTicker?.Dispose();
        _capacityTicker = null;

        try
        {
            Capacity?.SaveCalibration();
        }
        catch (Exception ex)
        {
            LogError("App.StopCapacityGovernor", ex); // calibration loss is recoverable.
        }

        Capacity = null;
        _capacityProvider?.Dispose();
        _capacityProvider = null;
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
            if (_window is not MainWindow mainWindow)
            {
                return;
            }

            _pipeServerEffects = new PipeServerEffects(mainWindow.WorkspaceHost, Authenticate);
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

    private Task<string?> HandleSocketLineAsync(string request, CancellationToken cancellationToken)
    {
        _ = cancellationToken;
        if (_pipeServerEffects is null)
        {
            return Task.FromResult<string?>(SocketWireProtocol.SerializeV1Response("ERROR: socket not ready"));
        }

        AuthState authState = _socketControlMode == SocketControlMode.Password
            ? new AuthState(RequiresAuthentication: true, IsAuthenticated: _socketAuthenticated)
            : AuthState.Unprotected;

        return Task.FromResult(CommandRouter.Dispatch(request, _pipeServerEffects, authState));
    }

    private bool Authenticate(string credential)
    {
        bool ok = _socketControlMode == SocketControlMode.Password
            ? _passwordStore.Verify(credential)
            : true;

        if (ok)
        {
            _socketAuthenticated = true;
        }

        return ok;
    }
}
