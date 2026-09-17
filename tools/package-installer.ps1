[CmdletBinding()]
param(
    # Staged portable package folder; defaults to the one tools/package.ps1 leaves in dist.
    [string]$PackageDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepositoryRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot "package-layout.ps1")
. (Join-Path $PSScriptRoot "signing.ps1")

$DistRoot = Join-Path $RepositoryRoot "dist"
if ([string]::IsNullOrWhiteSpace($PackageDirectory)) {
    $PackageDirectory = Join-Path $DistRoot $PackageName
}
if (-not (Test-Path -LiteralPath (Join-Path $PackageDirectory "FastPad.exe") -PathType Leaf)) {
    throw "Staged package '$PackageDirectory' was not found. Run tools/package.ps1 first."
}
$PackageDirectory = (Resolve-Path -LiteralPath $PackageDirectory).Path

function Find-InnoCompiler {
    $candidates = @(
        (Get-Command ISCC.exe -ErrorAction SilentlyContinue | ForEach-Object Source),
        (Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6\ISCC.exe"),
        (Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"),
        (Join-Path $env:ProgramFiles "Inno Setup 6\ISCC.exe")
    ) | Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Leaf) }
    if (@($candidates).Count -eq 0) {
        throw "ISCC.exe (Inno Setup 6) was not found. Install it with: winget install JRSoftware.InnoSetup"
    }
    return @($candidates)[0]
}

# VERSIONINFO needs four numeric parts; a pre-release suffix such as -rc.1 is dropped there only.
$numeric = @(($PackageVersion -replace '[-+].*$', '').Split('.') | ForEach-Object { [int]$_ })
while ($numeric.Count -lt 4) { $numeric += 0 }

$arguments = @(
    "/Q",
    "/DAppVersion=$PackageVersion",
    "/DNumericVersion=$($numeric -join '.')",
    "/DSourceDir=$PackageDirectory",
    "/DOutputDir=$DistRoot",
    "/DOutputName=$InstallerName"
)
if (Test-CodeSigningConfigured) {
    $arguments += @("/DSign", "/Sfastpad=$(Get-InnoSignToolCommand)")
}
else {
    Write-Warning "No code signing is configured (see tools/signing.ps1); the installer is unsigned."
}

$InstallerPath = Join-Path $DistRoot "$InstallerName.exe"
if (Test-Path -LiteralPath $InstallerPath) {
    Remove-Item -LiteralPath $InstallerPath -Force
}
& (Find-InnoCompiler) @arguments (Join-Path $RepositoryRoot "installer\FastPad.iss")
if ($LASTEXITCODE -ne 0) {
    throw "ISCC failed with exit code $LASTEXITCODE."
}
if (-not (Test-Path -LiteralPath $InstallerPath -PathType Leaf)) {
    throw "ISCC did not produce '$InstallerPath'."
}

$hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $InstallerPath).Hash.ToLowerInvariant()
Write-Output "Installer: $InstallerPath"
Write-Output "SHA-256: $hash"
