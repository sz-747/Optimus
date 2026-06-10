using System;
using System.IO;
using System.Security.Cryptography;
using System.Text;

namespace Optimus.Core;

/// <summary>
/// Pluggable secret protect/deprotect interface so Core can test DPAPI-style behavior without
/// hard dependency on a platform-specific implementation.
/// </summary>
public interface ISecretProtector
{
    byte[] Protect(byte[] plainText, byte[]? entropy = null);
    byte[] Unprotect(byte[] encryptedText, byte[]? entropy = null);
}

/// <summary>
/// Password source and verifier for the named-pipe socket.
/// Source precedence:
/// 1) OPTIMUS_SOCKET_PASSWORD env var
/// 2) DPAPI-backed file in %LOCALAPPDATA%\optimus\ (via <see cref="ISecretProtector"/>)
/// </summary>
public sealed class PasswordStore
{
    private const string PasswordFileName = "optimus-socket-password.bin";

    private readonly ISecretProtector _secretProtector;
    private readonly Func<string, string?> _getEnv;
    private readonly Func<string> _getLocalAppData;
    private readonly Func<string, bool> _fileExists;
    private readonly Func<string, byte[]> _readFile;

    public PasswordStore(
        ISecretProtector? secretProtector = null,
        Func<string, string?>? getEnv = null,
        Func<string>? getLocalAppData = null,
        Func<string, bool>? fileExists = null,
        Func<string, byte[]>? readFile = null)
    {
        _secretProtector = secretProtector ?? new NoopSecretProtector();
        _getEnv = getEnv ?? Environment.GetEnvironmentVariable;
        _getLocalAppData = getLocalAppData ?? (() => Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData));
        _fileExists = fileExists ?? File.Exists;
        _readFile = readFile ?? File.ReadAllBytes;
    }

    public bool Verify(string credential)
    {
        string? expected = ReadStoredPassword();
        if (string.IsNullOrEmpty(expected))
        {
            return false;
        }

        return ConstantTimeEquals(credential, expected);
    }

    public string? ReadStoredPassword()
    {
        string? envPassword = _getEnv(SocketAccess.PasswordEnvironmentVariable);
        if (!string.IsNullOrWhiteSpace(envPassword))
        {
            return envPassword;
        }

        string file = PasswordFilePath();
        if (!_fileExists(file))
        {
            return null;
        }

        try
        {
            byte[] encrypted = _readFile(file);
            byte[] plain = _secretProtector.Unprotect(encrypted);
            return Encoding.UTF8.GetString(plain).Trim();
        }
        catch
        {
            return null;
        }
    }

    private string PasswordFilePath() =>
        Path.Combine(_getLocalAppData(), "optimus", PasswordFileName);

    private static bool ConstantTimeEquals(string left, string right)
    {
        byte[] leftHash = SHA256.HashData(Encoding.UTF8.GetBytes(left));
        byte[] rightHash = SHA256.HashData(Encoding.UTF8.GetBytes(right));
        return CryptographicOperations.FixedTimeEquals(leftHash, rightHash);
    }

    private sealed class NoopSecretProtector : ISecretProtector
    {
        public byte[] Protect(byte[] plainText, byte[]? entropy = null) => plainText;
        public byte[] Unprotect(byte[] encryptedText, byte[]? entropy = null) => encryptedText;
    }
}
