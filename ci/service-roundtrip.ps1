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
    $line = (& $Sloop service status 2>$null | Select-String -Pattern '^\s*State\s+(.*)$').Matches
    if ($line.Count -ge 1) { return $line[0].Groups[1].Value.Trim() }
    return ''
}

function Invoke-Sloop {
    param([string[]] $SloopArguments)
    $previous = $LASTEXITCODE
    & $Sloop @SloopArguments 2>&1 | Out-String | Write-Verbose
    $code = $LASTEXITCODE
    $global:LASTEXITCODE = $previous
    return $code
}

# Left clean whatever happens: a half-installed service on a reused runner makes every later
# step lie.
function Remove-Everything {
    try { & $Sloop service uninstall 2>&1 | Out-Null } catch { }
}

try {
    Write-Host 'service-roundtrip: Windows, against the Service Control Manager'
    Write-Host ''

    # ----------------------------------------------------------------------- before

    CheckSame 'nothing installed, and it says so' (Get-State) 'not installed'
    CheckSame 'status exits 0 even with nothing installed' (Invoke-Sloop @('service', 'status')) 0
    CheckSame 'start refuses when there is nothing to start' (Invoke-Sloop @('service', 'start')) 2

    # ---------------------------------------------------------------------- install

    $log = & $Sloop service install 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) {
        Write-Host $log
        Write-Host 'service-roundtrip: install failed' -ForegroundColor Red
        exit 1
    }
    $log.TrimEnd() -split "`n" | ForEach-Object { Write-Host "  | $_" }
    Write-Host ''

    # The SCM's own answer, not sloop's: a service that never reported RUNNING would be
    # killed, and only the manager knows whether it did.
    $query = (sc.exe query sloop | Out-String)
    Check 'the Service Control Manager has it' { $query -notmatch '1060' }
    Check 'and it reached RUNNING under its own control handler' { $query -match 'RUNNING' }
    CheckSame 'sloop agrees it is running' (Get-State) 'running'
    Check 'the start type is AUTO_START, which is what a reboot depends on' {
        (sc.exe qc sloop | Out-String) -match 'AUTO_START'
    }

    Check 'the key file is there' { Test-Path -LiteralPath $key }
    # Rule 3: the service definition is readable by anybody who can run `sc qc`, so the
    # passphrase must not be in it.
    Check 'the service definition does not contain the passphrase' {
        $secret = Get-Content -LiteralPath $key -Raw
        -not ((sc.exe qc sloop | Out-String).Contains($secret.Trim()))
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
        (sc.exe qc sloop | Out-String) -match 'AUTO_START'
    }

    Invoke-Sloop @('service', 'start') | Out-Null
    CheckSame 'started again' (Get-State) 'running'
    CheckSame 'installing twice is not an error' (Invoke-Sloop @('service', 'install')) 0

    # --------------------------------------------------------------------- uninstall

    $log = & $Sloop service uninstall 2>&1 | Out-String
    $log.TrimEnd() -split "`n" | ForEach-Object { Write-Host "  | $_" }
    Write-Host ''

    Check 'the service entry is gone' { (sc.exe query sloop | Out-String) -match '1060' }
    Check 'the key file is gone' { -not (Test-Path -LiteralPath $key) }
    CheckSame 'and status says so' (Get-State) 'not installed'
    CheckSame 'uninstalling twice is not an error' (Invoke-Sloop @('service', 'uninstall')) 0

    Write-Host ''
    Write-Host "service-roundtrip: $passed of $checks checks passed."
} finally {
    Remove-Everything
}

exit $script:Failed
