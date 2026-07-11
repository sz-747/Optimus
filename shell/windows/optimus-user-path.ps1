[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Add', 'Remove', 'MigrateLegacy')]
    [string]$Action,

    [Parameter(Mandatory = $true)]
    [string]$Path,

    [string]$LegacyPath = '',
    [string]$PreviousPath = ''
)

$ErrorActionPreference = 'Stop'

function Normalize-PathSegment {
    param([AllowEmptyString()][string]$Value)

    if ([string]::IsNullOrWhiteSpace($Value)) {
        return ''
    }
    return $Value.Trim().TrimEnd('\')
}

function Test-SamePathSegment {
    param(
        [AllowEmptyString()][string]$Left,
        [AllowEmptyString()][string]$Right
    )

    return [string]::Equals(
        (Normalize-PathSegment $Left),
        (Normalize-PathSegment $Right),
        [System.StringComparison]::OrdinalIgnoreCase)
}

try {
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
    if ($null -eq $key) {
        throw 'could not open HKCU\Environment'
    }

    try {
        $current = $key.GetValue(
            'Path',
            '',
            [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        if ($null -eq $current) {
            $current = ''
        }
        $current = [string]$current

        $segments = if ($current.Length -eq 0) {
            @()
        }
        else {
            @($current.Split(
                [char[]]@(';'),
                [System.StringSplitOptions]::None))
        }
        $legacyPresent = $segments | Where-Object {
            Test-SamePathSegment $_ $LegacyPath
        } | Select-Object -First 1

        if ($Action -eq 'MigrateLegacy' -and $null -eq $legacyPresent) {
            exit 0
        }

        $remove = switch ($Action) {
            'Add' { @($Path, $LegacyPath, $PreviousPath) }
            'MigrateLegacy' { @($Path, $LegacyPath) }
            'Remove' { @($Path, $PreviousPath) }
        }

        $updated = [System.Collections.Generic.List[string]]::new()
        foreach ($segment in $segments) {
            $owned = $false
            foreach ($candidate in $remove) {
                if (-not [string]::IsNullOrWhiteSpace($candidate) -and
                    (Test-SamePathSegment $segment $candidate)) {
                    $owned = $true
                    break
                }
            }
            if (-not $owned) {
                $updated.Add($segment)
            }
        }

        if ($Action -ne 'Remove') {
            $updated.Add($Path)
        }

        $next = $updated -join ';'
        if ($next -ne $current) {
            $kind = try {
                $key.GetValueKind('Path')
            }
            catch {
                [Microsoft.Win32.RegistryValueKind]::ExpandString
            }
            if ($kind -notin @(
                [Microsoft.Win32.RegistryValueKind]::String,
                [Microsoft.Win32.RegistryValueKind]::ExpandString)) {
                $kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
            }
            $key.SetValue('Path', $next, $kind)
        }
    }
    finally {
        $key.Dispose()
    }

    if ($Action -eq 'MigrateLegacy') {
        exit 10
    }
    exit 0
}
catch {
    Write-Error $_
    exit 1
}
