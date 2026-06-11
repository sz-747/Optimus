namespace Optimus.Core;

/// <summary>
/// Persisted calibration record — the JSON shape of <c>capacity.json</c>:
/// <c>{ budgetBytes, hardwareFingerprintGb }</c>. The fingerprint is total physical RAM rounded
/// to whole GB, so a budget calibrated on one machine never poisons another.
/// </summary>
public sealed record CapacityCalibration(ulong BudgetBytes, int HardwareFingerprintGb);

/// <summary>
/// Load/save abstraction for <see cref="CapacityCalibration"/>. Core stays IO-free; the app
/// layer (U3) binds this to a JSON file next to existing app state.
/// </summary>
public interface ICalibrationStore
{
    /// <summary>The persisted calibration, or <c>null</c> if none exists or it is unreadable.</summary>
    CapacityCalibration? Load();

    void Save(CapacityCalibration calibration);
}
