using System;
using System.Collections.Generic;
using Optimus.Core;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Input;
using Windows.System;

namespace Optimus.Splits;

/// <summary>
/// Wires keyboard chords to controller operations (plan Phase 2 U5). A thin adapter: it builds one
/// <see cref="KeyboardAccelerator"/> per <see cref="ShortcutMap.Defaults"/> entry on the workspace
/// host, and routes each invocation through the pure <see cref="ShortcutMap.Apply"/> dispatcher
/// (the chord table and dispatch logic are unit-tested in <c>Optimus.Core</c>).
///
/// <para>Accelerators are scoped to the workspace host, so they fire even while a terminal surface
/// holds focus and are evaluated <i>before</i> the key reaches the terminal's <c>KeyDown</c> — the
/// accelerator marks the event handled, so a bound chord never leaks into the shell. Plain keys
/// (no modifier match) are untouched and flow to the focused terminal unchanged (R8).</para>
/// </summary>
internal sealed class ShortcutRouter
{
    private readonly SplitTreeController _controller;
    private readonly SurfaceManager _surfaces;
    // Host handler for the one chord whose effect lives in the surface plane, not the model plane:
    // opening a WebView2 pane (p6 U4). The model controller can only mint a kind-agnostic surface, so
    // the typed-surface intent is routed back to WorkspaceView. Null is tolerated (chord then no-ops).
    private readonly Action? _onNewWebPane;

    public ShortcutRouter(SplitTreeController controller, SurfaceManager surfaces, Action? onNewWebPane = null)
    {
        _controller = controller;
        _surfaces = surfaces;
        _onNewWebPane = onNewWebPane;
    }

    /// <summary>Register every default chord as a scoped accelerator on <paramref name="host"/>.</summary>
    public void Attach(UIElement host)
    {
        foreach (KeyValuePair<KeyChord, ShortcutAction> binding in ShortcutMap.Defaults)
        {
            ShortcutAction action = binding.Value;
            var accelerator = new KeyboardAccelerator
            {
                Key = (VirtualKey)binding.Key.KeyCode,
                Modifiers = ToVirtualModifiers(binding.Key.Modifiers),
            };
            accelerator.Invoked += (_, args) =>
            {
                args.Handled = true;
                Dispatch(action);
            };
            host.KeyboardAccelerators.Add(accelerator);
        }
    }

    private void Dispatch(ShortcutAction action)
    {
        // NewWebTab is host-handled (surface plane), not a model-plane controller op: route it to
        // WorkspaceView, which sets the pending surface kind and then asks the controller for a tab.
        if (action == ShortcutAction.NewWebTab)
        {
            _onNewWebPane?.Invoke();
            return;
        }

        ShortcutMap.Apply(_controller, action);

        // OS keyboard focus follows the derived model focus (R7/R8): after a focus or structural
        // change, push focus to the now-focused surface so keystrokes land in the right terminal.
        if (_controller.FocusedSurface is SurfaceId id)
        {
            _surfaces.Get(id)?.FocusSurface();
        }
    }

    private static VirtualKeyModifiers ToVirtualModifiers(ChordModifiers modifiers)
    {
        VirtualKeyModifiers result = VirtualKeyModifiers.None;
        if (modifiers.HasFlag(ChordModifiers.Ctrl))
        {
            result |= VirtualKeyModifiers.Control;
        }
        if (modifiers.HasFlag(ChordModifiers.Shift))
        {
            result |= VirtualKeyModifiers.Shift;
        }
        if (modifiers.HasFlag(ChordModifiers.Alt))
        {
            result |= VirtualKeyModifiers.Menu;
        }
        if (modifiers.HasFlag(ChordModifiers.Super))
        {
            result |= VirtualKeyModifiers.Windows;
        }
        return result;
    }
}
