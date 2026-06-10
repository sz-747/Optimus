namespace Optimus.Core;

/// <summary>
/// Auth state forwarded into <see cref="CommandRouter.Dispatch"/> so password-mode policies can be
/// layered without wiring auth into the router.
/// </summary>
public readonly record struct AuthState(bool RequiresAuthentication, bool IsAuthenticated)
{
    public static readonly AuthState Unprotected = new(false, false);
}

