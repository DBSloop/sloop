<#
.SYNOPSIS
    Register sloop with the Service Control Manager, drive it, and take it off again.

.DESCRIPTION
    Rule 14, and the Windows half is the one that cannot be checked any other way. A service
    the SCM accepts but that never reports SERVICE_RUNNING is killed about thirty seconds
    after it starts, and nothing in the source says whether the control handler is right --
    only the SCM's own answer does. So it is asked.

    What is asserted is the half that needs a real service manager: the SCM accepts the
    definition, the service reaches RUNNING under its own control handler, the start type is
    AUTO_START, stopped and never-installed come back as different answers, and uninstall
    leaves no service entry and no key file.

    What is not asserted, because a CI runner cannot be restarted: that it comes back after a
    reboot. AUTO_START is the thing a reboot would depend on, and that is checked.

    Needs administrator rights. A GitHub Windows runner has them; a developer's terminal
    needs 'Run as administrator'.

.PARAMETER Sloop
    The binary to drive. Defaults to cli\target\debug\sloop.exe.
#>

[CmdletBinding()]
param(
    [string] $Sloop
)

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
if (-not $Sloop) { $Sloop = Join-Path $root 'cli\target\debug\sloop.exe' }

if (-not (Test-Path -LiteralPath $Sloop)) {
    Write-Host "service-roundtrip: no binary at $Sloop" -ForegroundColor Red
    exit 1
}

$admin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()
         ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) {
    Write-Host 'service-roundtrip: registering a service is a machine-wide change.' -ForegroundColor Red
    Write-Host "  Run this from a terminal opened with 'Run as administrator'."
    exit 1
}

$key = Join-Path $env:ProgramData 'sloop\service.key'

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

# What `sloop service status` says on its State line.
function Get-State {
    $strict = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $line = (& $Sloop service status | Select-String -Pattern '^\s*State\s+(.*)$').Matches
        if ($line.Count -ge 1) { return $line[0].Groups[1].Value.Trim() }
        return ''
    } finally {
        $ErrorActionPreference = $strict
    }
}

# Everything a sloop command printed, both streams, without a refusal becoming fatal.
function Invoke-SloopSaying {
    param([string[]] $SloopArguments)

    $strict = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        return (& $Sloop @SloopArguments 2>&1 | Out-String)
    } finally {
        $ErrorActionPreference = $strict
    }
}

# `sc.exe`, likewise. Querying a service that does not exist is a *result* here rather than a
# fault -- it is how "the entry is gone" is checked -- and under `Stop` that would throw.
function Invoke-Sc {
    param([string[]] $ScArguments)

    $strict = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        return (& sc.exe @ScArguments 2>&1 | Out-String)
    } finally {
        $ErrorActionPreference = $strict
    }
}

# Run sloop and return only its exit code.
#
# **No `2>&1`, and `ErrorActionPreference` goes back to Continue for the call.** Windows
# PowerShell turns a native program's standard error into an ErrorRecord, and under `Stop`
# that is a terminating error -- so every sloop command that correctly printed a refusal would
# kill this script instead of being measured. Which is precisely what the commands under test
# are supposed to do.
function Invoke-Sloop {
    param([string[]] $SloopArguments)

    $strict = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $Sloop @SloopArguments | Out-Null
        return $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $strict
    }
}

# Left clean whatever happens: a half-installed service on a reused runner makes every later
# step lie.
function Remove-Everything {
    try { Invoke-SloopSaying @('service', 'uninstall') | Out-Null } catch { }
}

try {
    Write-Host 'service-roundtrip: Windows, against the Service Control Manager'
    Write-Host ''

    # ----------------------------------------------------------------------- before

    CheckSame 'nothing installed, and it says so' (Get-State) 'not installed'
    CheckSame 'status exits 0 even with nothing installed' (Invoke-Sloop @('service', 'status')) 0
    CheckSame 'start refuses when there is nothing to start' (Invoke-Sloop @('service', 'start')) 2

    # ---------------------------------------------------------------------- install

    $log = Invoke-SloopSaying @('service', 'install')
    if ($LASTEXITCODE -ne 0) {
        Write-Host $log
        Write-Host 'service-roundtrip: install failed' -ForegroundColor Red
        exit 1
    }
    $log.TrimEnd() -split "`n" | ForEach-Object { Write-Host "  | $_" }
    Write-Host ''

    # The SCM's own answer, not sloop's: a service that never reported RUNNING would be
    # killed, and only the manager knows whether it did.
    $query = Invoke-Sc @('query', 'sloop')
    Check 'the Service Control Manager has it' { $query -notmatch '1060' }
    Check 'and it reached RUNNING under its own control handler' { $query -match 'RUNNING' }
    CheckSame 'sloop agrees it is running' (Get-State) 'running'
    Check 'the start type is AUTO_START, which is what a reboot depends on' {
        (Invoke-Sc @('qc', 'sloop')) -match 'AUTO_START'
    }

    Check 'the key file is there' { Test-Path -LiteralPath $key }
    # Rule 3: the service definition is readable by anybody who can run `sc qc`, so the
    # passphrase must not be in it.
    Check 'the service definition does not contain the passphrase' {
        $secret = Get-Content -LiteralPath $key -Raw
        -not ((Invoke-Sc @('qc', 'sloop')).Contains($secret.Trim()))
    }
    # `Users` must not be able to read it. ProgramData grants them read by inheritance, so
    # this is the check that the inheritance was actually broken.
    Check 'and ordinary users cannot read it' {
        $acl = (icacls.exe $key | Out-String)
        -not ($acl -match 'BUILTIN\\Users' -or $acl -match '\\Users:')
    }

    # -------------------------------------------------------------------- stop, start

    Invoke-Sloop @('service', 'stop') | Out-Null
    CheckSame 'stopped, and not confused with never installed' (Get-State) 'installed, stopped'
    Check 'still registered for the next boot while stopped' {
        (Invoke-Sc @('qc', 'sloop')) -match 'AUTO_START'
    }

    Invoke-Sloop @('service', 'start') | Out-Null
    CheckSame 'started again' (Get-State) 'running'
    CheckSame 'installing twice is not an error' (Invoke-Sloop @('service', 'install')) 0

    # --------------------------------------------------------------------- uninstall

    $log = Invoke-SloopSaying @('service', 'uninstall')
    $log.TrimEnd() -split "`n" | ForEach-Object { Write-Host "  | $_" }
    Write-Host ''

    Check 'the service entry is gone' { (Invoke-Sc @('query', 'sloop')) -match '1060' }
    Check 'the key file is gone' { -not (Test-Path -LiteralPath $key) }
    CheckSame 'and status says so' (Get-State) 'not installed'
    CheckSame 'uninstalling twice is not an error' (Invoke-Sloop @('service', 'uninstall')) 0

    Write-Host ''
    Write-Host "service-roundtrip: $passed of $checks checks passed."
} finally {
    Remove-Everything
}

exit $script:Failed
