# READ-ONLY preflight for the disposable-admin #53 experiment. Writes nothing, launches nothing,
# never elevates (no Start-Process -Verb RunAs), reads no user profile / registry hive / keyring / .aleph.
# Usage (inside the disposable VM, already-elevated shell):  powershell -NoProfile -File preflight.ps1
# Exit 0 = every hard check passed; 1 = at least one hard check failed (do not run the experiment).
$ErrorActionPreference = 'Stop'
$fail = 0
function Row($name, $ok, $detail) {
  $tag = if ($ok) { 'PASS' } else { 'FAIL' }
  Write-Output ("{0,-4} {1,-34} {2}" -f $tag, $name, $detail)
  if (-not $ok) { $script:fail = 1 }
}
function Info($name, $detail) { Write-Output ("INFO {0,-34} {1}" -f $name, $detail) }

$id = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($id)
$admin = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
Row 'elevated (IsInRole Administrator)' $admin "IsInRole=$admin"

# Authoritative token-elevation + integrity level via whoami (read-only, own token only).
$groups = (& whoami /groups) -join "`n"
$high = $groups -match 'S-1-16-12288'
Row 'integrity level High (S-1-16-12288)' $high ''
$sys = $groups -match 'S-1-16-16384'
Info 'integrity level System' "$sys"

$bfe = Get-Service -Name BFE
Row 'BFE service Running' ($bfe.Status -eq 'Running') "Status=$($bfe.Status) StartType=$($bfe.StartType)"
$mpssvc = Get-Service -Name mpssvc
Info 'Windows Firewall service' "Status=$($mpssvc.Status)"

$os = Get-CimInstance Win32_OperatingSystem
Info 'OS' "$($os.Caption) build $($os.BuildNumber) arch $($os.OSArchitecture)"
$cs = Get-CimInstance Win32_ComputerSystem
$virt = [bool]$cs.HypervisorPresent
Row 'running under hypervisor (VM)' $virt "Manufacturer=$($cs.Manufacturer) Model=$($cs.Model)"
$memGB = [math]::Round($os.FreePhysicalMemory / 1MB, 1)
Row 'free RAM >= 4 GiB' ($memGB -ge 4) "$memGB GiB"

# Stale-state tripwire: any leftover Aleph experiment artefacts from a previous run (names only, no content).
$stale = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.ProcessName -match '^(aleph|sandbox-init|wfp_probe|job_probe)' })
Row 'no leftover aleph/probe processes' ($stale.Count -eq 0) (($stale | ForEach-Object { "$($_.ProcessName):$($_.Id)" }) -join ',')

# Required fixture binaries (must be built BEFORE the session; see windows-managed-admin-handoff.md section 5).
$root = Split-Path -Parent $PSCommandPath
foreach ($f in @('wfp_probe.exe', 'net_fixture.exe')) {
  $p = Join-Path $root $f
  Row "fixture present: $f" (Test-Path -LiteralPath $p -PathType Leaf) $p
}
Write-Output ''
Write-Output ("RESULT: " + $(if ($fail -eq 0) { 'ALL HARD CHECKS PASSED (necessary, NOT sufficient for #53)' } else { 'PREFLIGHT FAILED - do not run the experiment' }))
exit $fail
