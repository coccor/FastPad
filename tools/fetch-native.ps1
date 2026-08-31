[CmdletBinding()]
param(
    [switch]$VerifyOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepositoryRoot = Split-Path -Parent $PSScriptRoot
$NativeRoot = Join-Path $RepositoryRoot "native"
$CacheRoot = Join-Path $NativeRoot "cache"
$SourceRoot = Join-Path $NativeRoot "src"
$Manifest = Get-Content -Raw (Join-Path $NativeRoot "dependencies.json") | ConvertFrom-Json

$Dependencies = @(
    [pscustomobject]@{ Name = "scintilla"; Version = "5.6.6"; Archive = "scintilla566.zip"; VersionFile = "566" },
    [pscustomobject]@{ Name = "lexilla"; Version = "5.5.3"; Archive = "lexilla553.zip"; VersionFile = "553" }
)

function Assert-ArchiveHash {
    param(
        [Parameter(Mandatory = $true)] [string]$ArchivePath,
        [Parameter(Mandatory = $true)] [string]$ExpectedHash
    )

    $actualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $ArchivePath).Hash.ToLowerInvariant()
    if ($actualHash -ne $ExpectedHash.ToLowerInvariant()) {
        throw "SHA-256 mismatch for '$ArchivePath'. Expected $ExpectedHash; got $actualHash."
    }
}

function Assert-ExtractedVersion {
    param(
        [Parameter(Mandatory = $true)] [string]$Name,
        [Parameter(Mandatory = $true)] [string]$ExpectedVersion
    )

    $versionPath = Join-Path (Join-Path $SourceRoot $Name) "version.txt"
    if (-not (Test-Path -LiteralPath $versionPath -PathType Leaf)) {
        throw "Missing extracted version file '$versionPath'."
    }

    $actualVersion = (Get-Content -Raw -LiteralPath $versionPath).Trim()
    if ($actualVersion -ne $ExpectedVersion) {
        throw "Version mismatch for '$Name'. Expected $ExpectedVersion; got $actualVersion."
    }
}

foreach ($dependency in $Dependencies) {
    $manifestEntry = $Manifest.($dependency.Name)
    if ($null -eq $manifestEntry) {
        throw "Dependency manifest has no '$($dependency.Name)' entry."
    }
    if ($manifestEntry.version -ne $dependency.Version) {
        throw "Version mismatch in dependency manifest for '$($dependency.Name)'. Expected $($dependency.Version); got $($manifestEntry.version)."
    }

    $archivePath = Join-Path $CacheRoot $dependency.Archive
    if (-not (Test-Path -LiteralPath $archivePath -PathType Leaf)) {
        if ($VerifyOnly) {
            throw "Missing cached archive '$archivePath'; verification never downloads archives."
        }

        New-Item -ItemType Directory -Force -Path $CacheRoot | Out-Null
        Invoke-WebRequest -Uri $manifestEntry.url -OutFile $archivePath
    }

    Assert-ArchiveHash -ArchivePath $archivePath -ExpectedHash $manifestEntry.sha256

    if ($VerifyOnly) {
        Assert-ExtractedVersion -Name $dependency.Name -ExpectedVersion $dependency.VersionFile
        continue
    }

    New-Item -ItemType Directory -Force -Path $SourceRoot | Out-Null
    $sourcePath = Join-Path $SourceRoot $dependency.Name
    if (Test-Path -LiteralPath $sourcePath) {
        Remove-Item -LiteralPath $sourcePath -Recurse -Force
    }

    Expand-Archive -LiteralPath $archivePath -DestinationPath $SourceRoot -Force
    Assert-ExtractedVersion -Name $dependency.Name -ExpectedVersion $dependency.VersionFile
}
