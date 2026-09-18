<#
.SYNOPSIS
    Take sloop off a Windows machine -- the binary the installer wrote, and the PATH entry
    that points at it.

.DESCRIPTION
    irm https://dbsloop.github.io/uninstall.ps1 | iex

    `sloop uninstall` is the one to run first, and this says so rather than guessing. That
    command knows what sloop built on this machine -- a PostgreSQL it downloaded, a database,
    a role, a registry -- and it removes the binary and this PATH entry at the end of it.
    What is here is the fallback for a machine where the binary is gone, or broken, or was
    never on PATH: removing the file is all a script can honestly do, because a script
    cannot know which PostgreSQL was sloop's.

    So state still present, and a binary still able to remove it, is not something this
    decides on its own. It stops and names the parameter, exactly as the CLI would.

    Backups are never deleted. Not by this, not by `sloop uninstall`, not ever -- a registry
    is a minute of typing and a PostgreSQL is a download, but a dump that is gone is gone.

    PATH is edited through the registry API with expansion turned off, for the reason
    install.ps1 gives: reading a Path that holds %USERPROFILE% back expanded and writing it
    again replaces somebody else's variable with today's answer to it.

.PARAMETER Dir
    The directory sloop was installed into, if not %LOCALAPPDATA%\Programs\sloop\bin.

.PARAMETER BinaryOnly
    Leave sloop's database, registry and config alone.

.EXAMPLE
    irm https://dbsloop.github.io/uninstall.ps1 | iex

.EXAMPLE
    & ([scriptblock]::Create((irm https://dbsloop.github.io/uninstall.ps1))) -BinaryOnly
#>

[CmdletBinding()]
param(
    [string] $Dir = $env:SLOOP_INSTALL_DIR,
    [switch] $BinaryOnly
)

$ErrorActionPreference = 'Stop'

# The installer's note about this variable applies here too: the tests exercise the real
# removal against a throwaway key rather than the user's actual PATH.
$PathKey = if ($env:SLOOP_TEST_PATH_KEY) { $env:SLOOP_TEST_PATH_KEY } else { 'Environment' }

$script:Removed = $false

function Say([string] $Text) {
    Write-Host $Text
}

function Fail {
    param([Parameter(ValueFromRemainingArguments = $true)] [string[]] $Lines)

    Write-Host ''
    Write-Host '  sloop uninstall failed.' -ForegroundColor Red
    Write-Host ''
    foreach ($line in $Lines) { Write-Host "  $line" }
    Write-Host ''
    exit 1
}

# The one thing this script cannot decide on its own, said the way the CLI says it: what is
# in the way, and the parameter that answers it. Exit 2 is sloop's own code for a question a
# run could not be asked.
function Stop-Here {
    param([Parameter(ValueFromRemainingArguments = $true)] [string[]] $Lines)

    Write-Host ''
    Write-Host '  sloop uninstall stopped.' -ForegroundColor Yellow
    Write-Host ''
    foreach ($line in $Lines) { Write-Host "  $line" }
    Write-Host ''
    exit 2
}

function Get-UserPath {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($PathKey, $false)
    if (-not $key) { return $null }
    try {
        $value = $key.GetValue('Path', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        if ($null -eq $value) { return $null }
        return @{ Value = [string] $value; Kind = $key.GetValueKind('Path') }
    } catch {
        return $null
    } finally {
        $key.Close()
    }
}

function Publish-EnvironmentChange {
    try {
        if (-not ('Sloop.Broadcast' -as [type])) {
            Add-Type -Namespace 'Sloop' -Name 'Broadcast' -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true, CharSet = System.Runtime.InteropServices.CharSet.Auto)]
public static extern System.IntPtr SendMessageTimeout(
    System.IntPtr hWnd, uint Msg, System.UIntPtr wParam, string lParam,
    uint fuFlags, uint uTimeout, out System.UIntPtr lpdwResult);
'@
        }
        $result = [System.UIntPtr]::Zero
        [Sloop.Broadcast]::SendMessageTimeout(
            [System.IntPtr] 0xffff, 0x1A, [System.UIntPtr]::Zero, 'Environment',
            0x0002, 1000, [ref] $result) | Out-Null
    } catch {
        # Nothing to do about it, and nothing depends on it.
    }
}

# Take the entry out, matching on where it points rather than on how it is spelled.
#
# An entry written as %LOCALAPPDATA%\Programs\sloop\bin and one written out in full are the
# same directory, and an uninstall that only recognised its own spelling would leave the
# other behind -- including the one a person added by hand after installing somewhere else.
function Remove-FromPath([string] $Directory) {
    $current = Get-UserPath
    if (-not $current) { return }

    $wanted = $Directory.TrimEnd('\')
    $kept = @()
    $dropped = @()

    foreach ($entry in ($current.Value -split ';')) {
        if ($entry -eq '') { continue }
        $expanded = [Environment]::ExpandEnvironmentVariables($entry).TrimEnd('\')
        if ($expanded -ieq $wanted) { $dropped += $entry } else { $kept += $entry }
    }

    if ($dropped.Count -eq 0) { return }

    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($PathKey, $true)
    try {
        $key.SetValue('Path', ($kept -join ';'), $current.Kind)
    } finally {
        $key.Close()
    }
    Publish-EnvironmentChange

    foreach ($entry in $dropped) { Say "  removed PATH entry: $entry" }
    $script:Removed = $true
}

# ----------------------------------------------------------------------------------- run

if (-not $Dir) {
    if (-not $env:LOCALAPPDATA) {
        Fail 'LOCALAPPDATA is not set, so there is no default place to look in' `
            '  name the directory with -Dir <path>'
    }
    $Dir = Join-Path $env:LOCALAPPDATA 'Programs\sloop\bin'
}

$binary = Join-Path $Dir 'sloop.exe'

if (-not $BinaryOnly -and $env:APPDATA -and (Test-Path -LiteralPath $binary)) {
    $store = Join-Path $env:APPDATA 'sloop'
    if (Test-Path -LiteralPath $store) {
        Stop-Here "sloop still has state on this machine, and $binary can still remove it:" `
            '' `
            "  $store" `
            '' `
            '  Run this first -- it removes the PostgreSQL sloop installed, its database,' `
            '  its role and its registry, then the binary and the PATH entry:' `
            '' `
            "      `"$binary`" uninstall" `
            '' `
            '  Or leave that state where it is and take only the binary:' `
            '' `
            '      & ([scriptblock]::Create((irm https://dbsloop.github.io/uninstall.ps1))) -BinaryOnly' `
            '' `
            '  Backups are not deleted by either.'
    }
}

if (Test-Path -LiteralPath $binary) {
    try {
        Remove-Item -LiteralPath $binary -Force
    } catch {
        Fail "could not remove $binary" `
            '  a running sloop holds its own binary open -- close it and run this again'
    }
    Say "  removed $binary"
    $script:Removed = $true
}

# The directory the installer made, and only if the installer made it: everything under
# %LOCALAPPDATA%\Programs\sloop is sloop's, and an empty parent left behind is litter. A
# directory somebody chose with -Dir is theirs, and only the file goes.
$default = if ($env:LOCALAPPDATA) { Join-Path $env:LOCALAPPDATA 'Programs\sloop\bin' } else { $null }
if ($default -and ($Dir.TrimEnd('\') -ieq $default.TrimEnd('\'))) {
    $root = Split-Path -Parent $default
    if (Test-Path -LiteralPath $root) {
        $left = @(Get-ChildItem -LiteralPath $root -Recurse -Force -File -ErrorAction SilentlyContinue)
        if ($left.Count -eq 0) {
            Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
            Say "  removed $root"
            $script:Removed = $true
        } else {
            Say "  $root still holds files that are not sloop's -- left alone"
        }
    }
}

Remove-FromPath -Directory $Dir

Say ''
if ($script:Removed) {
    Say 'sloop is off this machine.'
    Say 'Open shells still have the old PATH; the next one will not.'
} else {
    Say 'Nothing to remove -- sloop was not installed by this installer.'
}
