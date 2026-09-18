<#
.SYNOPSIS
    Install sloop the way a stranger does, then take it off again, and fail the build if
    either half leaves something behind.

.DESCRIPTION
    Rule 14 is why this exists. An installer is the one piece of a release nobody runs until
    a stranger runs it, and "we tested it by hand once" is how a release ships a broken one.
    So it is driven here: a release layout is built from the binary this commit produced,
    install.ps1 is pointed at it, and what R21 promises is asserted -- the binary lands and
    runs, the PATH entry is written where a new shell reads it, and uninstall leaves nothing
    behind, that entry included.

    Nothing here touches the machine it runs on. The install goes into a temporary
    directory, and the PATH entry goes into a throwaway subkey of HKEY_CURRENT_USER named
    after this process -- never the real HKCU\Environment, which a failed run would then have
    left broken.

.PARAMETER Binary
    The sloop.exe to package. Defaults to cli\target\debug\sloop.exe.
#>

[CmdletBinding()]
param(
    [string] $Binary
)

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
if (-not $Binary) { $Binary = Join-Path $root 'cli\target\debug\sloop.exe' }

if (-not (Test-Path -LiteralPath $Binary)) {
    Write-Host "install-roundtrip: no binary at $Binary" -ForegroundColor Red
    Write-Host '  build one first: cargo build --manifest-path cli/Cargo.toml --bin sloop'
    exit 1
}

# The target triple, worked out a second time rather than asked of install.ps1. Two
# implementations that agree is a check; one agreeing with itself is not.
$architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
$target = switch -Regex ($architecture) {
    '^(X64|AMD64)$' { 'x86_64-pc-windows-msvc' }
    '^(Arm64|ARM64)$' { 'aarch64-pc-windows-msvc' }
    default {
        Write-Host "install-roundtrip: no sloop build for $architecture" -ForegroundColor Red
        exit 1
    }
}
$archive = "sloop-$target.zip"

$checks = 0
$passed = 0
$script:Failed = 0

function Record([bool] $Ok, [string] $Label) {
    $script:checks = $script:checks + 1
    if ($Ok) {
        $script:passed = $script:passed + 1
        Write-Host "  ok    $Label"
    } else {
        $script:Failed = 1
        Write-Host "  FAIL  $Label" -ForegroundColor Red
    }
}

function Check([string] $Label, [scriptblock] $Body) {
    $ok = $false
    try { $ok = [bool] (& $Body) } catch { $ok = $false }
    Record $ok $Label
}

function CheckSame([string] $Label, $Got, $Want) {
    if ($Got -eq $Want) { Record $true $Label } else { Record $false "$Label -- expected [$Want], got [$Got]" }
}

# Run one of the scripts in a PowerShell of its own, so an `exit` inside it ends that process
# rather than this one -- and so the PATH key it is told to use is set for it alone.
function Invoke-Script {
    param([string] $Script, [string[]] $ScriptArguments, [string] $PathKey, [string] $Log)

    $previous = $env:SLOOP_TEST_PATH_KEY
    $env:SLOOP_TEST_PATH_KEY = $PathKey
    try {
        $arguments = @(
            '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
            '-File', $Script
        ) + $ScriptArguments
        $run = Start-Process -FilePath 'powershell.exe' -ArgumentList $arguments `
            -NoNewWindow -Wait -PassThru -RedirectStandardOutput $Log `
            -RedirectStandardError "$Log.err"
        if (Test-Path -LiteralPath "$Log.err") {
            Get-Content -LiteralPath "$Log.err" | Add-Content -LiteralPath $Log
        }
        return $run.ExitCode
    } finally {
        $env:SLOOP_TEST_PATH_KEY = $previous
    }
}

function Get-TestPath([string] $PathKey) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($PathKey, $false)
    if (-not $key) { return $null }
    try {
        return [string] $key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    } finally {
        $key.Close()
    }
}

$pathKey = "Environment-sloop-roundtrip-$PID"
$work = Join-Path ([System.IO.Path]::GetTempPath()) "sloop-roundtrip-$PID"
$release = Join-Path $work 'release'
$staging = Join-Path $work 'staging'
$bin = Join-Path $work 'bin'

New-Item -ItemType Directory -Path $release, $staging, $bin -Force | Out-Null

try {
    # ------------------------------------------- a release, built the way R22 will build one

    Copy-Item -LiteralPath $Binary -Destination (Join-Path $staging 'sloop.exe')
    Copy-Item -LiteralPath (Join-Path $root 'LICENSE-MIT') -Destination $staging
    Copy-Item -LiteralPath (Join-Path $root 'LICENSE-APACHE') -Destination $staging
    Compress-Archive -Path (Join-Path $staging '*') -DestinationPath (Join-Path $release $archive) -Force

    $hash = (Get-FileHash -LiteralPath (Join-Path $release $archive) -Algorithm SHA256).Hash.ToLowerInvariant()
    Set-Content -LiteralPath (Join-Path $release 'SHA256SUMS') -Value "$hash  $archive" -Encoding ascii

    # A PATH that already holds an unexpanded variable, because that is the entry an
    # installer breaks: read it back expanded, write it again, and somebody else's
    # %USERPROFILE% has become today's answer to it.
    $seed = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($pathKey, $true)
    $seed.SetValue('Path', 'C:\Windows;%USERPROFILE%\bin', [Microsoft.Win32.RegistryValueKind]::ExpandString)
    $seed.Close()

    Write-Host "install-roundtrip: $target, from a release laid out in $release"
    Write-Host ''

    # ----------------------------------------------------------------------------- installing

    $log = Join-Path $work 'install.log'
    $code = Invoke-Script -Script (Join-Path $root 'install\install.ps1') `
        -ScriptArguments @('-From', $release, '-Dir', $bin) -PathKey $pathKey -Log $log
    Get-Content -LiteralPath $log | ForEach-Object { Write-Host "  | $_" }
    Write-Host ''

    if ($code -ne 0) {
        Write-Host 'install-roundtrip: install.ps1 failed' -ForegroundColor Red
        exit 1
    }

    $installed = Join-Path $bin 'sloop.exe'
    $text = (Get-Content -LiteralPath $log -Raw)

    Check 'the binary is installed' { Test-Path -LiteralPath $installed }
    Check 'the installed binary runs' { (& $installed --version) -ne $null }
    Check 'it says the shell has to be restarted' { $text -match 'Restart your shell' }

    $path = Get-TestPath $pathKey
    Check 'the PATH entry is in HKCU' {
        ($path -split ';' | Where-Object {
            [Environment]::ExpandEnvironmentVariables($_).TrimEnd('\') -ieq $bin.TrimEnd('\')
        }).Count -eq 1
    }
    CheckSame 'the entries that were already there keep their spelling' `
        (($path -split ';' | Select-Object -First 2) -join ';') 'C:\Windows;%USERPROFILE%\bin'

    # A new shell composes its PATH from this key at startup, so the entry being here is the
    # promise -- and it can be shown directly rather than inferred.
    $fresh = ($path -split ';' | Where-Object { $_ -ne '' } | ForEach-Object {
        [Environment]::ExpandEnvironmentVariables($_)
    }) -join ';'
    Check 'a shell built from that PATH finds sloop.exe' {
        ($fresh -split ';' | Where-Object { Test-Path -LiteralPath (Join-Path $_ 'sloop.exe') }).Count -ge 1
    }

    # Running it twice is running it once: an upgrade must not add a second entry.
    Invoke-Script -Script (Join-Path $root 'install\install.ps1') `
        -ScriptArguments @('-From', $release, '-Dir', $bin) -PathKey $pathKey `
        -Log (Join-Path $work 'again.log') | Out-Null
    $path = Get-TestPath $pathKey
    CheckSame 'installing twice leaves one entry, not two' `
        ($path -split ';' | Where-Object {
            [Environment]::ExpandEnvironmentVariables($_).TrimEnd('\') -ieq $bin.TrimEnd('\')
        }).Count 1

    # --------------------------------------------------------------------------- uninstalling

    $log = Join-Path $work 'uninstall.log'
    $code = Invoke-Script -Script (Join-Path $root 'install\uninstall.ps1') `
        -ScriptArguments @('-Dir', $bin, '-BinaryOnly') -PathKey $pathKey -Log $log
    Get-Content -LiteralPath $log | ForEach-Object { Write-Host "  | $_" }
    Write-Host ''

    CheckSame 'uninstall.ps1 succeeds' $code 0
    Check 'the binary is gone' { -not (Test-Path -LiteralPath $installed) }

    $path = Get-TestPath $pathKey
    CheckSame 'the PATH entry is gone' `
        ($path -split ';' | Where-Object {
            [Environment]::ExpandEnvironmentVariables($_).TrimEnd('\') -ieq $bin.TrimEnd('\')
        }).Count 0
    CheckSame 'and nothing else moved' $path 'C:\Windows;%USERPROFILE%\bin'

    $code = Invoke-Script -Script (Join-Path $root 'install\uninstall.ps1') `
        -ScriptArguments @('-Dir', $bin, '-BinaryOnly') -PathKey $pathKey `
        -Log (Join-Path $work 'uninstall-again.log')
    CheckSame 'uninstalling twice is not an error' $code 0
    Check 'the second uninstall says there was nothing to remove' {
        (Get-Content -LiteralPath (Join-Path $work 'uninstall-again.log') -Raw) -match 'Nothing to remove'
    }

    # ------------------------------------------------------------------------- the refusals

    # A tampered archive. The checksum is the only thing standing between a release and
    # whatever was actually served.
    $tampered = Join-Path $work 'tampered'
    New-Item -ItemType Directory -Path $tampered -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $release 'SHA256SUMS') -Destination $tampered
    Set-Content -LiteralPath (Join-Path $tampered $archive) -Value 'not the release you were looking for'

    $log = Join-Path $work 'tampered.log'
    $code = Invoke-Script -Script (Join-Path $root 'install\install.ps1') `
        -ScriptArguments @('-From', $tampered, '-Dir', (Join-Path $tampered 'bin')) `
        -PathKey $pathKey -Log $log
    CheckSame 'a tampered archive is refused' $code 1
    Check 'nothing is installed when the checksum does not match' {
        -not (Test-Path -LiteralPath (Join-Path $tampered 'bin\sloop.exe'))
    }
    Check 'the refusal says why' {
        (Get-Content -LiteralPath $log -Raw) -match 'not what that release published'
    }

    # A release that does not carry a build for this machine.
    $absent = Join-Path $work 'absent'
    New-Item -ItemType Directory -Path $absent -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $release $archive) -Destination $absent
    Set-Content -LiteralPath (Join-Path $absent 'SHA256SUMS') -Value ''

    $log = Join-Path $work 'absent.log'
    $code = Invoke-Script -Script (Join-Path $root 'install\install.ps1') `
        -ScriptArguments @('-From', $absent, '-Dir', (Join-Path $absent 'bin')) `
        -PathKey $pathKey -Log $log
    CheckSame 'a release with no checksum for this target is refused' $code 1
    Check 'it names the missing entry rather than a 404' {
        (Get-Content -LiteralPath $log -Raw) -match 'does not mention'
    }

    # -NoModifyPath means what it says.
    $quietKey = "$pathKey-quiet"
    $quiet = Join-Path $work 'quiet'
    $code = Invoke-Script -Script (Join-Path $root 'install\install.ps1') `
        -ScriptArguments @('-From', $release, '-Dir', $quiet, '-NoModifyPath') `
        -PathKey $quietKey -Log (Join-Path $work 'quiet.log')
    CheckSame '-NoModifyPath installs' $code 0
    Check '-NoModifyPath writes no PATH entry' { $null -eq (Get-TestPath $quietKey) }

    Write-Host ''
    Write-Host "install-roundtrip: $passed of $checks checks passed."
} finally {
    foreach ($key in @($pathKey, "$pathKey-quiet")) {
        try { [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKeyTree($key, $false) } catch { }
    }
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}

exit $script:Failed
