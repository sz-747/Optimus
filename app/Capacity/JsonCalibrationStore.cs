using System;
using System.IO;
using System.Text.Json;
using Optimus.Core;

namespace Optimus.Capacity;

/// <summary>
/// Persists <see cref="CapacityCalibration"/> as <c>capacity.json</c> in
/// <c>%LOCALAPPDATA%\optimus\</c> — the same directory the socket password store uses (see
/// <c>PasswordStore</c>). Shape: <c>{ "budgetBytes": ..., "hardwareFingerprintGb": ... }</c>.
/// Load is tolerant: any missing, corrupt, or unreadable file yields <c>null</c> so the model
/// falls back to the conservative seed budget.
/// </summary>
internal sealed class JsonCalibrationStore : ICalibrationStore
{
    private const string FileName = "capacity.json";

    private static readonly JsonSerializerOptions SerializerOptions = new()
    {
        PropertyNamingPolicy = JsonNamingPolicy.CamelCase,
        PropertyNameCaseInsensitive = true,
        WriteIndented = true,
    };

    private readonly string _filePath;

    public JsonCalibrationStore(string? directory = null)
    {
        directory ??= Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "optimus");
        _filePath = Path.Combine(directory, FileName);
    }

    /// <inheritdoc/>
    public CapacityCalibration? Load()
    {
        try
        {
            if (!File.Exists(_filePath))
            {
                return null;
            }
            var dto = JsonSerializer.Deserialize<CalibrationDto>(File.ReadAllText(_filePath), SerializerOptions);
            return dto is { BudgetBytes: > 0, HardwareFingerprintGb: > 0 }
                ? new CapacityCalibration(dto.BudgetBytes, dto.HardwareFingerprintGb)
                : null;
        }
        catch
        {
            return null; // corrupt or unreadable — start from the seed instead of crashing.
        }
    }

    /// <inheritdoc/>
    public void Save(CapacityCalibration calibration)
    {
        ArgumentNullException.ThrowIfNull(calibration);
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(_filePath)!);
            var dto = new CalibrationDto(calibration.BudgetBytes, calibration.HardwareFingerprintGb);
            File.WriteAllText(_filePath, JsonSerializer.Serialize(dto, SerializerOptions));
        }
        catch (Exception ex)
        {
            App.LogError("JsonCalibrationStore.Save", ex); // calibration loss is recoverable.
        }
    }

    private sealed record CalibrationDto(ulong BudgetBytes, int HardwareFingerprintGb);
}
