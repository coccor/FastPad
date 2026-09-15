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

$DownloadAttempts = 5
$DownloadTimeoutSeconds = 300
$InitialRetryDelaySeconds = 5

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

# Upstream hosting is slow and occasionally times out, so each attempt is bounded and retried with
# exponential backoff. A partial file never lands at the cached archive path.
function Invoke-ArchiveDownload {
    param(
        [Parameter(Mandatory = $true)] [string]$Uri,
        [Parameter(Mandatory = $true)] [string]$Destination
    )

    $partialPath = "$Destination.partial"
    $delaySeconds = $InitialRetryDelaySeconds
    for ($attempt = 1; $attempt -le $DownloadAttempts; $attempt++) {
        try {
            Invoke-WebRequest -Uri $Uri -OutFile $partialPath -TimeoutSec $DownloadTimeoutSeconds
            Move-Item -LiteralPath $partialPath -Destination $Destination -Force
            return
        }
        catch {
            Remove-Item -LiteralPath $partialPath -Force -ErrorAction SilentlyContinue
            if ($attempt -eq $DownloadAttempts) {
                throw "Downloading '$Uri' failed after $DownloadAttempts attempts: $($_.Exception.Message)"
            }
            Write-Warning "Download attempt $attempt of $DownloadAttempts for '$Uri' failed: $($_.Exception.Message). Retrying in $delaySeconds s."
            Start-Sleep -Seconds $delaySeconds
            $delaySeconds *= 2
        }
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
    $downloaded = $false
    if (-not (Test-Path -LiteralPath $archivePath -PathType Leaf)) {
        if ($VerifyOnly) {
            throw "Missing cached archive '$archivePath'; verification never downloads archives."
        }

        New-Item -ItemType Directory -Force -Path $CacheRoot | Out-Null
        Invoke-ArchiveDownload -Uri $manifestEntry.url -Destination $archivePath
        $downloaded = $true
    }

    try {
        Assert-ArchiveHash -ArchivePath $archivePath -ExpectedHash $manifestEntry.sha256
    }
    catch {
        # A freshly downloaded archive with the wrong hash must not poison the cache for the next run.
        if ($downloaded) {
            Remove-Item -LiteralPath $archivePath -Force -ErrorAction SilentlyContinue
        }
        throw
    }

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
