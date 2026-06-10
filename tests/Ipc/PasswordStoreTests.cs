using System;
using System.Text;
using Optimus.Core;
using Xunit;

namespace Optimus.Core.Tests;

public sealed class PasswordStoreTests
{
    [Fact] // Covers R3 priority.
    public void Environment_password_takes_precedence_over_file()
    {
        var protector = new TrackingSecretProtector();
        PasswordStore store = new PasswordStore(
            secretProtector: protector,
            getEnv: _ => " env-pass ",
            getLocalAppData: () => @"C:\users\app\Local",
            fileExists: _ => true,
            readFile: _ => Array.Empty<byte>());

        Assert.True(store.Verify(" env-pass "));
        Assert.False(protector.UnprotectCalled);
    }

    [Fact] // Covers R3 file source + verify.
    public void Stored_file_password_is_verified_and_used()
    {
        var protector = new TrackingSecretProtector();
        string expected = "from-file";
        PasswordStore store = new PasswordStore(
            secretProtector: protector,
            getEnv: _ => null,
            getLocalAppData: () => @"C:\users\app\Local",
            fileExists: path => string.Equals(path, @"C:\users\app\Local\optimus\optimus-socket-password.bin"),
            readFile: path => protector.Protect(Encoding.UTF8.GetBytes(expected)));

        Assert.True(store.Verify(expected));
        Assert.False(store.Verify("bad"));
        Assert.True(protector.UnprotectCalled);
    }

    [Fact] // Covers R3 missing password path.
    public void Verify_is_false_when_no_password_is_available()
    {
        PasswordStore store = new PasswordStore(
            secretProtector: new TrackingSecretProtector(),
            getEnv: _ => null,
            getLocalAppData: () => @"C:\users\app\Local",
            fileExists: _ => false);

        Assert.False(store.Verify("anything"));
    }

    private sealed class TrackingSecretProtector : ISecretProtector
    {
        public bool UnprotectCalled { get; private set; }

        public byte[] Protect(byte[] plainText, byte[]? entropy = null) => plainText;
        public byte[] Unprotect(byte[] encryptedText, byte[]? entropy = null)
        {
            UnprotectCalled = true;
            return encryptedText;
        }
    }
}
