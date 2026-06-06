using System;
using System.IO;
using System.IO.Pipes;
using System.Security.AccessControl;
using System.Security.Principal;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using Cmux.Core;

namespace Cmux.Ipc;

/// <summary>
/// Named-pipe server owner for the app side of the CLI socket path.
/// Runs connection accepts on a background task and writes framed responses
/// back to each client on the same stream.
/// </summary>
internal sealed class PipeServer
{
    private const int MaxConcurrentClients = 16;
    private const int MaxServerInstances = 16;
    private const string DefaultVariant = "stable";

    private readonly Func<string, CancellationToken, Task<string?>> _dispatch;
    private readonly Func<string, StreamWriter, CancellationToken, Task>? _eventsStream;
    private readonly string _pipeName;
    private readonly int _maxServerInstances;
    private readonly SocketControlMode _controlMode;
    private readonly Func<string?, bool> _isClientSidAllowed;
    private readonly SemaphoreSlim _clientSlots;
    private readonly PipeSecurity _pipeSecurity;
    private readonly CancellationTokenSource _stopping = new();
    private readonly object _state = new();
    private Task? _acceptLoop;
    private bool _isRunning;

    public PipeServer(
        Func<string, CancellationToken, Task<string?>> dispatch,
        Func<string, StreamWriter, CancellationToken, Task>? eventsStream = null,
        SocketControlMode controlMode = SocketControlMode.CmuxOnly,
        string? pipeName = null,
        int maxConcurrentClients = MaxConcurrentClients,
        Func<string?, bool>? clientSidValidator = null)
    {
        _dispatch = dispatch ?? throw new ArgumentNullException(nameof(dispatch));
        _eventsStream = eventsStream;
        _controlMode = controlMode;
        _pipeName = ResolvePipeName(pipeName);
        _maxServerInstances = Math.Max(1, maxConcurrentClients);
        _clientSlots = new SemaphoreSlim(Math.Max(1, maxConcurrentClients));
        _pipeSecurity = BuildPipeSecurity();
        _isClientSidAllowed = clientSidValidator ?? (_ => true);
    }

    public bool IsRunning
    {
        get
        {
            lock (_state)
            {
                return _isRunning && _acceptLoop is not null && !_acceptLoop.IsCompleted;
            }
        }
    }

    public void Start()
    {
        lock (_state)
        {
            if (_isRunning)
            {
                return;
            }
            _isRunning = true;
            _acceptLoop = Task.Run(() => AcceptLoopAsync(_stopping.Token));
            _acceptLoop.ContinueWith(
                static task =>
                {
                    if (task.IsFaulted)
                    {
                        App.LogError("PipeServer.AcceptLoop", task.Exception);
                    }
                },
                TaskScheduler.Default);
        }
    }

    public void Stop()
    {
        lock (_state)
        {
            if (!_isRunning)
            {
                return;
            }
            _isRunning = false;
        }

        _stopping.Cancel();
    }

    private async Task AcceptLoopAsync(CancellationToken token)
    {
        try
        {
            while (!token.IsCancellationRequested)
            {
                await _clientSlots.WaitAsync(token).ConfigureAwait(false);
                NamedPipeServerStream? server = null;
                try
                {
                    server = CreateServerStream();
                    await server.WaitForConnectionAsync(token).ConfigureAwait(false);
                    _ = HandleClientAsync(server, token);
                }
                catch (OperationCanceledException)
                {
                    server?.Dispose();
                    break;
                }
                catch (Exception ex)
                {
                    server?.Dispose();
                    _clientSlots.Release();
                    App.LogError("PipeServer.AcceptLoop", ex);
                    break;
                }
            }
        }
        catch (OperationCanceledException)
        {
            // expected during shutdown
        }
        finally
        {
            lock (_state)
            {
                _isRunning = false;
            }
        }
    }

    private async Task HandleClientAsync(NamedPipeServerStream server, CancellationToken token)
    {
        try
        {
            if (!IsClientAuthorized(server))
            {
                await using StreamWriter unauthorized = new(server, Encoding.UTF8, bufferSize: 4096, leaveOpen: true) { AutoFlush = true };
                await WriteResponseAsync(unauthorized, SocketWireProtocol.SerializeV1Response("ERROR: access denied"));
                return;
            }

            using StreamReader reader = new(server, Encoding.UTF8, detectEncodingFromByteOrderMarks: true, bufferSize: 4096, leaveOpen: true);
            using StreamWriter writer = new(server, Encoding.UTF8, bufferSize: 4096, leaveOpen: true) { AutoFlush = true };

            while (!token.IsCancellationRequested && server.IsConnected)
            {
                string? request = await reader.ReadLineAsync(token).ConfigureAwait(false);
                if (request is null)
                {
                    break;
                }

                if (request.Length == 0)
                {
                    continue;
                }

                if (IsEventsStreamRequest(request))
                {
                    if (_eventsStream is null)
                    {
                        await WriteResponseAsync(writer, "ERROR: events.stream is not supported yet");
                        continue;
                    }

                    await _eventsStream(request, writer, token).ConfigureAwait(false);
                    break;
                }

                string? response = await _dispatch(request, token).ConfigureAwait(false);
                if (response is not null)
                {
                    await WriteResponseAsync(writer, response).ConfigureAwait(false);
                }
            }
        }
        catch (OperationCanceledException)
        {
            // expected on shutdown
        }
        catch (Exception ex)
        {
            App.LogError("PipeServer.HandleClientAsync", ex);
        }
        finally
        {
            _clientSlots.Release();
            server.Dispose();
        }
    }

    private bool IsClientAuthorized(NamedPipeServerStream server)
    {
        if (!SocketAccess.RequiresPeerSidCheck(_controlMode))
        {
            return true;
        }

        string? peerSid = PeerIdentity.ResolveClientSid(server);
        return _isClientSidAllowed(peerSid);
    }

    private bool IsEventsStreamRequest(string request)
    {
        if (SocketWireProtocol.IsV2Frame(request))
        {
            try
            {
                return SocketWireProtocol.ParseV2(request).Method == SocketMethods.EventsStreamV2;
            }
            catch
            {
                // leave parse errors to the command router.
                return false;
            }
        }

        return SocketWireProtocol.ParseV1(request).Verb == SocketMethods.EventsStream;
    }

    private static async Task WriteResponseAsync(StreamWriter writer, string payload)
    {
        string response = payload.EndsWith('\n') ? payload : payload + '\n';
        await writer.WriteAsync(response).ConfigureAwait(false);
    }

    private static PipeSecurity BuildPipeSecurity()
    {
        PipeSecurity pipeSecurity = new();
        SecurityIdentifier? userSid = WindowsIdentity.GetCurrent()?.User;
        if (userSid is null)
        {
            return pipeSecurity;
        }

        pipeSecurity.SetOwner(userSid);
        pipeSecurity.SetAccessRuleProtection(isProtected: true, preserveInheritance: false);
        pipeSecurity.AddAccessRule(new PipeAccessRule(userSid, PipeAccessRights.FullControl, AccessControlType.Allow));
        return pipeSecurity;
    }

    private NamedPipeServerStream CreateServerStream()
    {
        return NamedPipeServerStreamAcl.Create(
            pipeName: _pipeName,
            direction: PipeDirection.InOut,
            maxNumberOfServerInstances: _maxServerInstances,
            transmissionMode: PipeTransmissionMode.Byte,
            options: PipeOptions.Asynchronous,
            inBufferSize: 0,
            outBufferSize: 0,
            pipeSecurity: _pipeSecurity,
            inheritability: HandleInheritability.None);
    }

    private static string ResolvePipeName(string? configuredName)
    {
        if (!string.IsNullOrWhiteSpace(configuredName))
        {
            return configuredName;
        }

        string? envName = PipeName.ResolveFromEnvironment(Environment.GetEnvironmentVariable);
        if (!string.IsNullOrWhiteSpace(envName))
        {
            return envName;
        }

        return PipeName.BuildPipeName(DefaultVariant);
    }
}
