using System;
using System.ComponentModel;

namespace Optimus.Core;

/// <summary>
/// View-model behind the always-visible chrome capacity indicator (RAM safe-zone plan U6):
/// "X / Y terminals" plus a thin level-colored bar above the New-Workspace button. Lives in Core
/// (not the WinUI assembly) so it is unit-testable without a dispatcher: UI-thread marshalling is
/// an injected <c>Action&lt;Action&gt;</c>, and the view maps <see cref="Level"/> to design tokens.
///
/// <para>Trusts <see cref="CapacityState.Level"/> as computed by <see cref="CapacityModel"/> —
/// never recomputes thresholds. A <c>null</c> model (governor failed to start, plan U3) renders
/// as the dashed placeholder and never blocks spawning from the chrome side.</para>
/// </summary>
public sealed class CapacityIndicatorViewModel : INotifyPropertyChanged, IDisposable
{
    /// <summary>One-line reason shown under the disabled New-Workspace button at cap.</summary>
    public const string CapHint = "Safe-zone full — close a workspace to spawn more";

    private readonly CapacityModel? _model;
    private readonly Action<Action> _dispatch;
    private CapacityState? _state;
    private bool _disposed;

    /// <param name="model">The capacity governor, or <c>null</c> when it failed to start.</param>
    /// <param name="dispatch">Marshals to the UI thread; <see cref="CapacityModel.StateChanged"/>
    /// fires on background/timer threads (plan U3).</param>
    public CapacityIndicatorViewModel(CapacityModel? model, Action<Action> dispatch)
    {
        _model = model;
        _dispatch = dispatch ?? throw new ArgumentNullException(nameof(dispatch));
        if (model is not null)
        {
            _state = model.State;
            model.StateChanged += OnStateChanged;
        }
    }

    public event PropertyChangedEventHandler? PropertyChanged;

    /// <summary>"X / Y terminals" where X = Used + Reserved; "— / — terminals" with no model.</summary>
    public string LabelText =>
        _state is CapacityState s ? $"{s.Used + s.Reserved} / {s.Max} terminals" : "— / — terminals";

    /// <summary>Fill fraction for the bar, clamped to [0, 1]; 0 when Max is 0 or no model.</summary>
    public double FractionUsed
    {
        get
        {
            if (_state is not CapacityState s || s.Max <= 0)
            {
                return 0.0;
            }
            return Math.Clamp((double)(s.Used + s.Reserved) / s.Max, 0.0, 1.0);
        }
    }

    /// <summary>Escalation level straight from the model; Calm when no model.</summary>
    public CapacityLevel Level => _state?.Level ?? CapacityLevel.Calm;

    /// <summary>True at the safe-zone cap — the chrome disables the New-Workspace affordance.</summary>
    public bool IsAtCap => Level == CapacityLevel.Cap;

    /// <summary>The cap hint, or <c>null</c> below the cap (the view hides the line).</summary>
    public string? HintText => IsAtCap ? CapHint : null;

    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }
        _disposed = true;
        if (_model is not null)
        {
            _model.StateChanged -= OnStateChanged;
        }
    }

    private void OnStateChanged(CapacityState state) => _dispatch(() =>
    {
        if (_disposed)
        {
            return;
        }
        _state = state;
        Raise(nameof(LabelText));
        Raise(nameof(FractionUsed));
        Raise(nameof(Level));
        Raise(nameof(IsAtCap));
        Raise(nameof(HintText));
    });

    private void Raise(string name) => PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));
}
