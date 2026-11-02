[CmdletBinding()]
param(
    [ValidateSet('install','update','rollback','inspect')][string]$Action='install',
    [string]$InstanceHome=(Join-Path $env:LOCALAPPDATA 'AgentInstance\first-agent'),
    [string]$Binary,
    [string]$Name='New agent',
    [string]$BindAddress='0.0.0.0',
    [int]$Port=0,
    [string]$Release,
    [switch]$NoService,
    [switch]$NoBrowser,
    [switch]$SkipRuntime,
    [ValidateSet('native','codex')][string]$Engine='native',
    [string]$RuntimeBinary,
    [switch]$SkipPrerequisites
)
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
function Install-Prerequisite([string]$Id, [string[]]$Extra=@()) {
    if ($SkipPrerequisites) { throw "Missing prerequisite $Id. Install it or supply -Binary." }
    if (-not (Get-Command winget -ErrorAction SilentlyContinue)) { throw "Install $Id, then rerun. Windows App Installer (winget) is unavailable." }
    & winget install --id $Id --exact --silent --accept-package-agreements --accept-source-agreements @Extra | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "Prerequisite installation failed: $Id" }
    $env:Path=[Environment]::GetEnvironmentVariable('Path','Machine')+';'+[Environment]::GetEnvironmentVariable('Path','User')
}
function Find-Python {
    $candidate=Get-Command python -ErrorAction SilentlyContinue
    if ($candidate -and $candidate.Source -notlike '*WindowsApps*') { return $candidate.Source }
    return $null
}
$Python=Find-Python
if (-not $Python) { Install-Prerequisite 'Python.Python.3.13'; $Python=Find-Python }
if (-not $Python) { throw 'Python 3 is installed; open a fresh PowerShell window and run this command again.' }
if (($Action -eq 'install' -or $Action -eq 'update') -and -not $Binary) {
    $Cargo=Get-Command cargo -ErrorAction SilentlyContinue
    if (-not $Cargo -and (Test-Path "$env:USERPROFILE\.cargo\bin\cargo.exe")) { $Cargo=Get-Item "$env:USERPROFILE\.cargo\bin\cargo.exe" }
    if (-not $Cargo) {
        Install-Prerequisite 'Rustlang.Rustup'
        $Cargo=Get-Item "$env:USERPROFILE\.cargo\bin\cargo.exe" -ErrorAction SilentlyContinue
    }
    if (-not $Cargo) { throw 'Rust is installed; open a fresh PowerShell window and retry.' }
    $Vswhere="${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    $BuildTools=if (Test-Path $Vswhere) { & $Vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath }
    if (-not $BuildTools) { Install-Prerequisite 'Microsoft.VisualStudio.2022.BuildTools' @('--override','--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended') }
    $CargoPath=if ($Cargo.Source) { $Cargo.Source } else { $Cargo.FullName }
    Push-Location $PSScriptRoot
    try {
        & $CargoPath build --release --locked | Out-Host
        if ($LASTEXITCODE -ne 0) { throw 'Native Rust build failed; see the build output.' }
    } finally { Pop-Location }
    $Binary=Join-Path $PSScriptRoot 'target\release\agent-instance.exe'
}
$Options=@((Join-Path $PSScriptRoot 'scripts\native-install.py'),$Action,'--home',$InstanceHome,'--name',$Name,'--bind',$BindAddress,'--port',"$Port")
if ($Binary) { $Options+=@('--binary',$Binary) }
if ($Release) { $Options+=@('--release',$Release) }
if ($NoService) { $Options+='--no-service' }
if ($NoBrowser) { $Options+='--no-browser' }
if ($SkipRuntime) { $Options+='--skip-runtime' }
$Options+=@('--engine',$Engine)
if ($RuntimeBinary) { $Options+=@('--runtime-binary',$RuntimeBinary) }
& $Python @Options
exit $LASTEXITCODE
