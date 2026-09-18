<#
.SYNOPSIS
    Install sloop on Windows.

.DESCRIPTION
    irm https://dbsloop.github.io/install.ps1 | iex

    It never asks anything. The command above pipes this file into `iex`, so there is no
    console to read an answer from, and rule 4 forbids a prompt without a terminal anyway.
    Every decision is made from the machine or from a parameter, and anything that cannot be
    decided that way fails naming the parameter that would have decided it.

    The binary it downloads holds no network code, and neither does this: the one fetch
    happens here, through the system's own Invoke-WebRequest, before the binary exists. What
    arrives is proved against SHA256SUMS from the same release before a byte of it is
    unpacked.

    This file is ASCII, and has to stay ASCII. Windows PowerShell 5.1 reads a .ps1 without
    a byte-order mark as the system's ANSI codepage, so a UTF-8 em dash arrives as three
    CP1252 characters - and one of them is U+201D, which PowerShell accepts as a string
    delimiter. A comment with a dash in it then ends a string early and the script dies on a
    brace mismatch fifty lines away. A BOM would fix the file on disk and break it through
    `irm | iex`, so the rule is the other one: no character above 0x7E, checked by
    ci/ascii-scripts.sh.

    PATH is written to HKCU\Environment directly rather than through
    [Environment]::SetEnvironmentVariable, and the reason is not style. That API reads the
    value back expanded -- a Path holding %USERPROFILE%\bin comes back as C:\Users\...\bin, and
    writing that back replaces a variable somebody else's installer put there with today's
    answer to it. The registry API can be told not to expand, and is.

.PARAMETER Version
    A release other than the latest, as 1.2.3 or v1.2.3.

.PARAMETER Dir
    Somewhere other than %LOCALAPPDATA%\Programs\sloop\bin.

.PARAMETER From
    A URL or a directory to fetch from instead of GitHub releases. The installer's own
    tests point this at a release layout built locally.

.PARAMETER NoModifyPath
    Install, and leave PATH alone.

.EXAMPLE
    irm https://dbsloop.github.io/install.ps1 | iex

.EXAMPLE
    Passing a parameter through the pipe needs the script to become a scriptblock first:

    & ([scriptblock]::Create((irm https://dbsloop.github.io/install.ps1))) -Version 1.2.3
#>

[CmdletBinding()]
param(
    [string] $Version = $env:SLOOP_VERSION,
    [string] $Dir = $env:SLOOP_INSTALL_DIR,
    [string] $From = $env:SLOOP_INSTALL_BASE,
    [switch] $NoModifyPath
)

$ErrorActionPreference = 'Stop'
# TLS 1.2 is not the default on Windows PowerShell 5.1, and GitHub serves nothing older.
# Without this the download fails with a connection error that says nothing about why.
try {
    [Net.ServicePointManager]::SecurityProtocol =
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch {
    # .NET 5+ hosts have no ServicePointManager to set and need none.
}

$RepoUrl = 'https://github.com/DBSloop/sloop'
$ReleasesUrl = "$RepoUrl/releases"

# Where the PATH entry lives. The subkey is a variable for one reason: the installer's own
# tests exercise the real code against a throwaway key instead of the user's actual PATH,
# and a test that edits HKCU\Environment to prove itself is a test that can break the
# machine it runs on.
$PathKey = if ($env:SLOOP_TEST_PATH_KEY) { $env:SLOOP_TEST_PATH_KEY } else { 'Environment' }

function Say([string] $Text) {
    Write-Host $Text
}

function Fail {
    param([Parameter(ValueFromRemainingArguments = $true)] [string[]] $Lines)

    Write-Host ''
    Write-Host '  sloop install failed.' -ForegroundColor Red
    Write-Host ''
    foreach ($line in $Lines) { Write-Host "  $line" }
    Write-Host ''
    exit 1
}

# ------------------------------------------------------------------------------- machine

# Which release archive this machine takes.
#
# Named as Rust names it, because the target triple is what the release workflow builds with
# and what a person reading the assets page will recognise. RuntimeInformation is asked
# before PROCESSOR_ARCHITECTURE, because a 32-bit PowerShell on a 64-bit machine reports x86
# from the environment variable and the truth from the API.
function Get-Target {
    $architecture = $null
    try {
        $architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    } catch {
        $architecture = $env:PROCESSOR_ARCHITEW6432
        if (-not $architecture) { $architecture = $env:PROCESSOR_ARCHITECTURE }
    }

    switch -Regex ($architecture) {
        '^(X64|AMD64)$' { return 'x86_64-pc-windows-msvc' }
        '^(Arm64|ARM64)$' { return 'aarch64-pc-windows-msvc' }
        default {
            Fail "sloop has no build for $architecture" `
                "  the releases page lists what there is: $ReleasesUrl"
        }
    }
}

# ------------------------------------------------------------------------------ fetching

# Fetch one file, from a URL or from a directory.
#
# A directory is what the installer's own tests point at: the release layout built locally,
# so the whole path below -- checksum, unpack, install, PATH -- is exercised on a runner with
# no release to download.
function Get-File([string] $Source, [string] $Destination) {
    if ($Source -match '^https?://') {
        try {
            # -UseBasicParsing matters on 5.1: without it the cmdlet initialises Internet
            # Explorer's engine, which is absent on Server Core and on a fresh user profile.
            Invoke-WebRequest -Uri $Source -OutFile $Destination -UseBasicParsing
        } catch {
            Fail "could not download $Source" "  $($_.Exception.Message)"
        }
    } else {
        $local = $Source -replace '^file:///?', ''
        if (-not (Test-Path -LiteralPath $local)) {
            Fail "could not read $local"
        }
        Copy-Item -LiteralPath $local -Destination $Destination -Force
    }
}

# Prove the archive is the one the release published, before anything is unpacked.
function Confirm-Checksum([string] $File, [string] $Name, [string] $Sums) {
    $expected = $null
    foreach ($line in (Get-Content -LiteralPath $Sums)) {
        $fields = $line -split '\s+', 2
        if ($fields.Count -lt 2) { continue }
        $candidate = $fields[1].Trim().TrimStart('*')
        if ($candidate -eq $Name) { $expected = $fields[0].Trim().ToLowerInvariant(); break }
    }

    if (-not $expected) {
        Fail "SHA256SUMS from that release does not mention $Name" `
            "  the release may not carry a build for this machine -- see $ReleasesUrl"
    }

    $actual = (Get-FileHash -LiteralPath $File -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        Fail "$Name is not what that release published" `
            "  expected $expected" `
            "  received $actual" `
            "  nothing has been installed. Try again; if it happens twice, say so at $RepoUrl/issues"
    }

    Say '  checksum verified'
}

# --------------------------------------------------------------------------- installing

# Put the binary where it goes.
#
# A running sloop.exe cannot be overwritten -- Windows holds an open image file locked, and
# the copy fails with a message about another process. That is said in those words rather
# than left as a raw exception, because "close the sloop that is running" is a fix somebody
# can act on and "access to the path is denied" is not.
function Install-Binary([string] $From, [string] $To) {
    $parent = Split-Path -Parent $To
    New-Item -ItemType Directory -Path $parent -Force | Out-Null

    try {
        Copy-Item -LiteralPath $From -Destination $To -Force
    } catch {
        Fail "could not write $To" `
            '  a running sloop holds its own binary open -- close it and run this again,' `
            '  or install somewhere else with -Dir <path>'
    }

    Say "  installed $To"
}

# --------------------------------------------------------------------------------- PATH

# The user's PATH, exactly as it is stored.
#
# DoNotExpandEnvironmentNames is the whole point: a Path holding %USERPROFILE%\bin has to
# come back with the %USERPROFILE% still in it, or writing it again would bake today's
# answer into somebody else's entry. The value kind is read for the same reason and put
# back unchanged.
function Get-UserPath {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($PathKey, $false)
    if (-not $key) { return @{ Value = ''; Kind = [Microsoft.Win32.RegistryValueKind]::ExpandString } }
    try {
        $value = $key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $kind = $key.GetValueKind('Path')
        return @{ Value = [string] $value; Kind = $kind }
    } catch {
        return @{ Value = ''; Kind = [Microsoft.Win32.RegistryValueKind]::ExpandString }
    } finally {
        $key.Close()
    }
}

function Set-UserPath([string] $Value, $Kind) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($PathKey, $true)
    try {
        $key.SetValue('Path', $Value, $Kind)
    } finally {
        $key.Close()
    }
    Publish-EnvironmentChange
}

# Tell the rest of Windows that the environment moved.
#
# Explorer and everything launched from it read the environment once and cache it; without
# the broadcast a new terminal opened from the Start menu still has yesterday's PATH. It is
# best effort -- a machine where the P/Invoke will not compile is a machine where the user
# restarts a shell, which they are told to do regardless.
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
        # HWND_BROADCAST, WM_SETTINGCHANGE, SMTO_ABORTIFHUNG, one second.
        [Sloop.Broadcast]::SendMessageTimeout(
            [System.IntPtr] 0xffff, 0x1A, [System.UIntPtr]::Zero, 'Environment',
            0x0002, 1000, [ref] $result) | Out-Null
    } catch {
        # Nothing to do about it, and nothing depends on it.
    }
}

function Add-ToPath([string] $Directory) {
    $current = Get-UserPath
    $entries = @($current.Value -split ';' | Where-Object { $_ -ne '' })

    # Compared after expansion, so an entry written as %LOCALAPPDATA%\Programs\sloop\bin is
    # recognised as the same directory and not added a second time.
    foreach ($entry in $entries) {
        $expanded = [Environment]::ExpandEnvironmentVariables($entry).TrimEnd('\')
        if ($expanded -ieq $Directory.TrimEnd('\')) {
            Say "  $Directory is already on PATH"
            return $false
        }
    }

    # Written with %LOCALAPPDATA% rather than the expanded path when it is under it, so the
    # entry keeps working on a machine where that is spelled differently -- a roaming
    # profile, a renamed account, a restored image.
    $written = $Directory
    $localAppData = $env:LOCALAPPDATA
    if ($localAppData -and $Directory.StartsWith($localAppData, [StringComparison]::OrdinalIgnoreCase)) {
        $written = '%LOCALAPPDATA%' + $Directory.Substring($localAppData.Length)
    }

    $entries += $written
    $kind = $current.Kind
    if ($written -like '*%*') {
        # A REG_SZ holding %LOCALAPPDATA% is a literal percent sign to every process that
        # reads it. The value kind has to say it expands, or the entry is a path that does
        # not exist.
        $kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
    }

    Set-UserPath -Value ($entries -join ';') -Kind $kind
    Say "  PATH entry added: $written"
    return $true
}

# ----------------------------------------------------------------------------------- run

if ($Version) { $Version = $Version -replace '^v', '' }

$target = Get-Target
$archive = "sloop-$target.zip"
$binary = 'sloop.exe'

if (-not $Dir) {
    if (-not $env:LOCALAPPDATA) {
        Fail 'LOCALAPPDATA is not set, so there is no default place to install into' `
            '  name one with -Dir <path>'
    }
    $Dir = Join-Path $env:LOCALAPPDATA 'Programs\sloop\bin'
}

if (-not $From) {
    # `latest/download` is a redirect GitHub serves itself, so the newest release needs no
    # API call, no JSON and no token.
    $From = if ($Version) { "$ReleasesUrl/download/v$Version" } else { "$ReleasesUrl/latest/download" }
}

$shown = if ($Version) { $Version } else { 'latest' }
Say "sloop $shown for $target"

$temp = Join-Path ([System.IO.Path]::GetTempPath()) ("sloop-install-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $temp -Force | Out-Null

try {
    $archivePath = Join-Path $temp $archive
    $sumsPath = Join-Path $temp 'SHA256SUMS'

    Get-File -Source "$From/$archive" -Destination $archivePath
    Get-File -Source "$From/SHA256SUMS" -Destination $sumsPath
    Confirm-Checksum -File $archivePath -Name $archive -Sums $sumsPath

    $unpacked = Join-Path $temp 'unpacked'
    try {
        Expand-Archive -LiteralPath $archivePath -DestinationPath $unpacked -Force
    } catch {
        Fail "could not unpack $archive" "  $($_.Exception.Message)"
    }

    $source = Join-Path $unpacked $binary
    if (-not (Test-Path -LiteralPath $source)) {
        Fail "$archive does not hold a $binary" "  download by hand from $ReleasesUrl"
    }

    $installed = Join-Path $Dir $binary
    Install-Binary -From $source -To $installed

    # `$ErrorActionPreference` goes back to Continue for this one call. Windows PowerShell
    # turns a native program's stderr into an ErrorRecord, and under Stop that would make a
    # binary which merely printed a warning look like one that cannot run.
    $versionLine = ''
    try {
        $strict = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        $versionLine = (& $installed --version | Select-Object -First 1)
        $ErrorActionPreference = $strict
    } catch {
        $versionLine = ''
    }
    if (-not $versionLine) {
        Fail "$installed does not run on this machine" `
            "  the archive for $target was installed -- if that is the wrong architecture," `
            "  the releases page lists the rest: $ReleasesUrl"
    }

    $changed = $false
    $alreadyThere = $false
    if ($NoModifyPath) {
        Say '  PATH untouched, as asked'
    } else {
        $changed = Add-ToPath -Directory $Dir
        $alreadyThere = -not $changed
    }

    Say ''
    Say "$versionLine is installed."
    Say ''
    if ($changed) {
        # A shell reads its environment once, when it starts. Saying this after somebody has
        # already hit "sloop is not recognized" is saying it too late.
        Say 'Restart your shell, or open a new terminal, before running sloop.'
        Say 'This one read its PATH when it started and will not see the new entry.'
        Say ''
        Say 'To use it in this window without restarting:'
        Say "    `$env:Path = `"$Dir;`$env:Path`""
    } elseif ($alreadyThere) {
        Say 'Run: sloop'
    } else {
        Say "Run: $installed"
    }
    Say ''
    Say 'Then: sloop setup'
} finally {
    Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue
}
