using System;
using System.Threading;
using Microsoft.Win32.SafeHandles;
using Optimus.Core;

namespace Optimus.Capacity;

/// <summary>
/// Drives the capacity governor (RAM safe-zone plan U3): a 1 Hz <see cref="Timer"/> calls
/// <see cref="CapacityModel.OnTick"/>, and a thread-pool wait on the OS
/// low-memory-resource-notification handle fires the provider's <c>LowMemorySignal</c> plus an
/// immediate tick so pressure tightens the cap within ~1 s instead of waiting for the next beat.
///
/// The notification object is level-triggered (stays signaled while memory is low), so the wait
/// is registered <c>executeOnlyOnce</c> and re-armed from the timer tick only after the signal
/// clears — otherwise the thread pool would spin on the still-signaled handle. While the signal
/// is pending re-arm, the model still sees pressure via <c>IsLowMemorySignaled</c> polling each
/// tick, so nothing is missed.
/// </summary>
internal sealed class CapacityTicker : IDisposable
{
    private static readonly TimeSpan TickInterval = TimeSpan.FromSeconds(1);

    private readonly CapacityModel _model;
    private readonly Win32CapacityProvider _provider;
    private readonly object _gate = new();
    private readonly Timer _timer;
    private WaitHandle? _notificationEvent;
    private RegisteredWaitHandle? _registeredWait;
    private bool _waitFired;
    private bool _disposed;

    public CapacityTicker(CapacityModel model, Win32CapacityProvider provider)
    {
        _model = model ?? throw new ArgumentNullException(nameof(model));
        _provider = provider ?? throw new ArgumentNullException(nameof(provider));
        _timer = new Timer(Tick, state: null, dueTime: TickInterval, period: TickInterval);
        ArmLowMemoryWait();
    }

    private void Tick(object? state)
    {
        try
        {
            _model.OnTick();
            RearmLowMemoryWaitIfCleared();
        }
        catch (Exception ex)
        {
            App.LogError("CapacityTicker.Tick", ex);
        }
    }

    private void OnLowMemorySignaled(object? state, bool timedOut)
    {
        if (timedOut)
        {
            return;
        }
        try
        {
            lock (_gate)
            {
                if (_disposed)
                {
                    return;
                }
                _waitFired = true;
            }
            _provider.RaiseLowMemorySignal();
            _model.OnTick();
        }
        catch (Exception ex)
        {
            App.LogError("CapacityTicker.OnLowMemorySignaled", ex);
        }
    }

    /// <summary>Register a one-shot thread-pool wait on the notification handle.</summary>
    private void ArmLowMemoryWait()
    {
        IntPtr handle = _provider.NotificationHandle;
        if (handle == IntPtr.Zero)
        {
            return; // creation failed — 1 Hz polling still covers pressure, just slower.
        }

        lock (_gate)
        {
            if (_disposed || _registeredWait is not null)
            {
                return;
            }
            // Borrow (do not own) the provider's handle so disposing the event never closes it.
            _notificationEvent ??= new BorrowedWaitHandle(handle);
            _waitFired = false;
            _registeredWait = ThreadPool.RegisterWaitForSingleObject(
                _notificationEvent,
                OnLowMemorySignaled,
                state: null,
                millisecondsTimeOutInterval: -1,
                executeOnlyOnce: true);
        }
    }

    /// <summary>Re-arm the one-shot wait once the level-triggered signal has cleared.</summary>
    private void RearmLowMemoryWaitIfCleared()
    {
        bool shouldRearm;
        lock (_gate)
        {
            shouldRearm = !_disposed && _waitFired && !_provider.IsLowMemorySignaled;
            if (shouldRearm)
            {
                _registeredWait?.Unregister(null);
                _registeredWait = null;
            }
        }
        if (shouldRearm)
        {
            ArmLowMemoryWait();
        }
    }

    public void Dispose()
    {
        RegisteredWaitHandle? wait;
        WaitHandle? notificationEvent;
        lock (_gate)
        {
            if (_disposed)
            {
                return;
            }
            _disposed = true;
            wait = _registeredWait;
            _registeredWait = null;
            notificationEvent = _notificationEvent;
            _notificationEvent = null;
        }

        // Drain the timer before tearing down the wait so no tick re-arms mid-dispose.
        using (var drained = new ManualResetEvent(initialState: false))
        {
            if (_timer.Dispose(drained))
            {
                drained.WaitOne(TimeSpan.FromSeconds(2));
            }
        }

        // Drain the wait too: Unregister(waitObject) signals `callbacksDone` once any in-flight
        // OnLowMemorySignaled callback has completed, so the provider/model it touches are not
        // disposed underneath it (bounded — never hang shutdown on a stuck callback).
        if (wait is not null)
        {
            using var callbacksDone = new ManualResetEvent(initialState: false);
            if (wait.Unregister(callbacksDone))
            {
                callbacksDone.WaitOne(TimeSpan.FromSeconds(2));
            }
        }
        notificationEvent?.Dispose(); // does not close the provider's handle (ownsHandle: false)
    }

    /// <summary>
    /// A <see cref="WaitHandle"/> view over a borrowed kernel handle. Unlike
    /// <c>new ManualResetEvent(false) { SafeWaitHandle = ... }</c>, this never creates a kernel
    /// event of its own (which would leak when its SafeWaitHandle is replaced), and
    /// <c>ownsHandle: false</c> means disposing it never closes the provider's handle.
    /// </summary>
    private sealed class BorrowedWaitHandle : WaitHandle
    {
        public BorrowedWaitHandle(IntPtr handle)
        {
            SafeWaitHandle = new SafeWaitHandle(handle, ownsHandle: false);
        }
    }
}
