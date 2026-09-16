[CmdletBinding()]
param(
    [string]$Installer,
    [switch]$RequireSignature
)

# Installs the setup executable silently for the current user into a temporary folder, checks the
# installed files and per-user registration, then uninstalls and checks that everything is gone.
# It refuses to run over an existing FastPad installation.

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepositoryRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot "package-layout.ps1")
if ([string]::IsNullOrWhiteSpace($Installer)) {
    $Installer = Join-Path $RepositoryRoot "dist\$InstallerName.exe"
}
if (-not (Test-Path -LiteralPath $Installer -PathType Leaf)) {
    throw "Installer '$Installer' does not exist. Run tools/package-installer.ps1 first."
}

$UninstallKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{DD2BB0BB-6510-4E72-922F-75CC10D1244E}_is1"
$AppPathsKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths\FastPad.exe"
$ProgIdKey = "HKCU:\Software\Classes\FastPad.Document"
$ApplicationKey = "HKCU:\Software\Classes\Applications\FastPad.exe"
$ContextMenuKey = "HKCU:\Software\Classes\*\shell\FastPad"
$TxtOpenWithKey = "HKCU:\Software\Classes\.txt\OpenWithProgids"
$StartMenuShortcut = Join-Path ([Environment]::GetFolderPath("Programs")) "FastPad.lnk"

if (Test-Path -LiteralPath $UninstallKey) {
    throw "FastPad is already installed for this user; uninstall it before verifying an installer."
}

function Wait-Until {
    param([string]$What, [scriptblock]$Condition, [int]$TimeoutSeconds = 60)
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while (-not (& $Condition)) {
        if ([DateTime]::UtcNow -ge $deadline) { throw "Timed out waiting for $What." }
        Start-Sleep -Milliseconds 200
    }
}

function Test-OpenWithProgId {
    $key = Get-Item -LiteralPath $TxtOpenWithKey -ErrorAction SilentlyContinue
    return $null -ne $key -and $key.GetValueNames() -contains "FastPad.Document"
}

if ($RequireSignature) {
    $signature = Get-AuthenticodeSignature -LiteralPath $Installer
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
        throw "The installer does not have a valid Authenticode signature ($($signature.Status): $($signature.StatusMessage))."
    }
    Write-Output "Installer Authenticode signature is valid."
}

$VerifyRoot = Join-Path ([System.IO.Path]::GetTempPath()) "fastpad-installer-$([guid]::NewGuid().ToString('N'))"
$InstallDir = Join-Path $VerifyRoot "FastPad"
$installed = $false
try {
    New-Item -ItemType Directory -Force -Path $VerifyRoot | Out-Null
    $log = Join-Path $VerifyRoot "install.log"
    $setup = Start-Process -FilePath $Installer -Wait -PassThru -ArgumentList @(
        "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CURRENTUSER",
        "/TASKS=contextmenu", "/DIR=`"$InstallDir`"", "/LOG=`"$log`"")
    $installed = $true
    if ($setup.ExitCode -ne 0) {
        throw "Setup exited with code $($setup.ExitCode); see '$log'."
    }

    $actual = @(Get-ChildItem -LiteralPath $InstallDir -Recurse -File |
        ForEach-Object { [System.IO.Path]::GetRelativePath($InstallDir, $_.FullName) } |
        Where-Object { $_ -notmatch '^unins\d{3}\.(exe|dat)$' } |
        Sort-Object)
    $expected = @($PackageFiles | Sort-Object)
    if ((@($actual) -join "|") -ne ($expected -join "|")) {
        throw "Installed files are wrong. Expected [$($expected -join ', ')]; got [$($actual -join ', ')]."
    }
    Write-Output "Installed exactly the package files into '$InstallDir'."

    $registration = Get-ItemProperty -LiteralPath $UninstallKey
    if ($registration.DisplayVersion -ne $PackageVersion) {
        throw "Uninstall entry reports version '$($registration.DisplayVersion)', expected '$PackageVersion'."
    }
    $exe = Join-Path $InstallDir "FastPad.exe"
    if ((Get-ItemProperty -LiteralPath $AppPathsKey)."(default)" -ne $exe) {
        throw "App Paths does not point at '$exe'."
    }
    foreach ($key in @($ProgIdKey, $ApplicationKey, $ContextMenuKey)) {
        if (-not (Test-Path -LiteralPath $key)) { throw "Registry key '$key' was not written." }
    }
    if (-not (Test-OpenWithProgId)) { throw ".txt does not offer FastPad.Document in Open with." }
    if (-not (Test-Path -LiteralPath $StartMenuShortcut -PathType Leaf)) {
        throw "Start menu shortcut '$StartMenuShortcut' was not created."
    }
    Write-Output "Per-user registration (uninstall entry, App Paths, Open with, context menu, Start menu) is present."

    $uninstaller = Join-Path $InstallDir "unins000.exe"
    $uninstall = Start-Process -FilePath $uninstaller -Wait -PassThru -ArgumentList @(
        "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART")
    if ($uninstall.ExitCode -ne 0) {
        throw "Uninstaller exited with code $($uninstall.ExitCode)."
    }
    # The uninstaller re-launches itself from %TEMP% and returns before it has finished.
    Wait-Until "the uninstaller to finish" {
        -not (Test-Path -LiteralPath $UninstallKey) -and -not (Test-Path -LiteralPath $exe)
    }
    $installed = $false
    foreach ($key in @($AppPathsKey, $ProgIdKey, $ApplicationKey, $ContextMenuKey)) {
        if (Test-Path -LiteralPath $key) { throw "Uninstall left registry key '$key'." }
    }
    if (Test-OpenWithProgId) { throw "Uninstall left FastPad.Document in .txt Open with." }
    if (Test-Path -LiteralPath $StartMenuShortcut) { throw "Uninstall left the Start menu shortcut." }
    Write-Output "Uninstall removed the files and every per-user registration."
}
finally {
    if ($installed -and (Test-Path -LiteralPath (Join-Path $InstallDir "unins000.exe"))) {
        Write-Warning "Verification failed while installed; uninstalling."
        Start-Process -FilePath (Join-Path $InstallDir "unins000.exe") -Wait -ArgumentList @(
            "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART") | Out-Null
        Start-Sleep -Seconds 3
    }
    Remove-Item -LiteralPath $VerifyRoot -Recurse -Force -ErrorAction SilentlyContinue
}
