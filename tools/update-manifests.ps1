[CmdletBinding()]
param(
    # Release version without the leading "v"; defaults to the Cargo.toml version.
    [string]$Version,
    # SHA256SUMS.txt from the GitHub release ("<sha256>  <file name>" per line).
    [Parameter(Mandatory = $true)] [string]$Sha256Sums,
    [string]$ReleaseDate = (Get-Date -Format "yyyy-MM-dd")
)

# Renders the package-manager manifests for a published GitHub release:
#   bucket/fastpad.json                  Scoop (this repository is the bucket)
#   dist/winget/<version>/coccor.FastPad*.yaml  winget, ready for `wingetcreate submit`

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepositoryRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot "package-layout.ps1")
if ([string]::IsNullOrWhiteSpace($Version)) {
    $Version = $PackageVersion
}
$Version = $Version.TrimStart("v")
if ($Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$') {
    throw "'$Version' is not a semantic version."
}

$Repository = "https://github.com/coccor/FastPad"
$Publisher = "Cocioaba Cornel"
$PackageIdentifier = "coccor.FastPad"
$ProductCode = "{DD2BB0BB-6510-4E72-922F-75CC10D1244E}_is1"
$ShortDescription = "A startup-latency-first text editor for Windows"
$Description = "FastPad is a native Win32 text editor built on Scintilla, with no UI framework, browser runtime, background service, or network access. The window accepts typing before settings, file loading, and highlighting finish."
$ZipName = "FastPad-$Version-windows-x64.zip"
$SetupName = "FastPad-$Version-windows-x64-setup.exe"
$DownloadRoot = "$Repository/releases/download/v$Version"

$hashes = @{}
foreach ($line in Get-Content -LiteralPath $Sha256Sums) {
    if ($line -match '^([0-9A-Fa-f]{64})\s+\*?(.+?)\s*$') {
        $hashes[$Matches[2]] = $Matches[1].ToLowerInvariant()
    }
}
foreach ($name in @($ZipName, $SetupName)) {
    if (-not $hashes.ContainsKey($name)) {
        throw "'$Sha256Sums' has no SHA-256 for '$name'."
    }
}

$scoop = [ordered]@{
    version      = $Version
    description  = $ShortDescription
    homepage     = $Repository
    license      = "MIT"
    architecture = [ordered]@{
        "64bit" = [ordered]@{
            url  = "$DownloadRoot/$ZipName"
            hash = $hashes[$ZipName]
        }
    }
    bin          = "FastPad.exe"
    shortcuts    = @(, @("FastPad.exe", "FastPad"))
    checkver     = "github"
    autoupdate   = [ordered]@{
        architecture = [ordered]@{
            "64bit" = [ordered]@{
                url = "$Repository/releases/download/v`$version/FastPad-`$version-windows-x64.zip"
            }
        }
        hash         = [ordered]@{ url = "`$baseurl/SHA256SUMS.txt" }
    }
}
$bucket = Join-Path $RepositoryRoot "bucket"
New-Item -ItemType Directory -Force -Path $bucket | Out-Null
$scoopPath = Join-Path $bucket "fastpad.json"
Set-Content -LiteralPath $scoopPath -Value ($scoop | ConvertTo-Json -Depth 8) -Encoding utf8NoBOM
Write-Output "Scoop: $scoopPath"

$ManifestVersion = "1.10.0"
$wingetRoot = Join-Path $RepositoryRoot "dist\winget\$Version"
New-Item -ItemType Directory -Force -Path $wingetRoot | Out-Null

$versionManifest = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.version.$ManifestVersion.schema.json
PackageIdentifier: $PackageIdentifier
PackageVersion: $Version
DefaultLocale: en-US
ManifestType: version
ManifestVersion: $ManifestVersion
"@

$installerManifest = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.installer.$ManifestVersion.schema.json
PackageIdentifier: $PackageIdentifier
PackageVersion: $Version
MinimumOSVersion: 10.0.0.0
InstallerType: inno
Scope: user
InstallModes:
- interactive
- silent
- silentWithProgress
UpgradeBehavior: install
FileExtensions:
- ini
- json
- log
- markdown
- md
- txt
ProductCode: '$ProductCode'
ReleaseDate: $ReleaseDate
AppsAndFeaturesEntries:
- DisplayName: FastPad
  Publisher: $Publisher
  ProductCode: '$ProductCode'
Installers:
- Architecture: x64
  InstallerUrl: $DownloadRoot/$SetupName
  InstallerSha256: $($hashes[$SetupName].ToUpperInvariant())
ManifestType: installer
ManifestVersion: $ManifestVersion
"@

$localeManifest = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.defaultLocale.$ManifestVersion.schema.json
PackageIdentifier: $PackageIdentifier
PackageVersion: $Version
PackageLocale: en-US
Publisher: $Publisher
PublisherUrl: https://github.com/coccor
PublisherSupportUrl: $Repository/issues
PackageName: FastPad
PackageUrl: $Repository
License: MIT
LicenseUrl: $Repository/blob/v$Version/LICENSE
ShortDescription: $ShortDescription
Description: $Description
Moniker: fastpad
Tags:
- editor
- json
- markdown
- notepad
- text-editor
ReleaseNotesUrl: $Repository/releases/tag/v$Version
ManifestType: defaultLocale
ManifestVersion: $ManifestVersion
"@

foreach ($entry in @(
        @{ Name = "$PackageIdentifier.yaml"; Content = $versionManifest },
        @{ Name = "$PackageIdentifier.installer.yaml"; Content = $installerManifest },
        @{ Name = "$PackageIdentifier.locale.en-US.yaml"; Content = $localeManifest })) {
    Set-Content -LiteralPath (Join-Path $wingetRoot $entry.Name) -Value $entry.Content -Encoding utf8NoBOM
}
Write-Output "winget: $wingetRoot"
