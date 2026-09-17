[CmdletBinding()]
param(
    [ValidateSet("release", "release-size", "release-thin")]
    [string]$BuildProfile = "release"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepositoryRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot "msvc.ps1")
. (Join-Path $PSScriptRoot "package-layout.ps1")
. (Join-Path $PSScriptRoot "signing.ps1")

$Target = "x86_64-pc-windows-msvc"
$DistRoot = Join-Path $RepositoryRoot "dist"
# Kept apart from native\out\x64, which holds the development test fixtures.
$NativeOutput = Join-Path $RepositoryRoot "native\out\package-x64"
$TargetDirectory = Join-Path $RepositoryRoot "target\package"
$StageRoot = Join-Path $DistRoot $PackageName
$ZipPath = Join-Path $DistRoot "$PackageName.zip"
$HashPath = Join-Path $DistRoot "$PackageName.sha256"
$CargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $HOME ".cargo" }

& (Join-Path $PSScriptRoot "build-native.ps1") -OutputDirectory $NativeOutput
if (-not $?) {
    throw "Native build from pinned sources failed."
}

$previousEncodedRustFlags = $env:CARGO_ENCODED_RUSTFLAGS
# 0x1F-separated so paths with spaces survive. A static CRT avoids requiring the VC++ redistributable;
# the remaps keep this checkout and the Cargo registry path out of panic locations.
$env:CARGO_ENCODED_RUSTFLAGS = @(
    "-Ctarget-feature=+crt-static",
    "--remap-path-prefix=$RepositoryRoot=fastpad",
    "--remap-path-prefix=$CargoHome=cargo"
) -join [char]0x1F
Push-Location $RepositoryRoot
try {
    & cargo build --locked --profile $BuildProfile --features release-package --bin fastpad --target $Target --target-dir $TargetDirectory
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build --profile $BuildProfile --features release-package failed."
    }
}
finally {
    Pop-Location
    $env:CARGO_ENCODED_RUSTFLAGS = $previousEncodedRustFlags
}

$RustCrateLicenses = Join-Path $DistRoot "generated\rust-crates.txt"
& (Join-Path $PSScriptRoot "rust-crate-licenses.ps1") -OutputPath $RustCrateLicenses
if (-not $?) {
    throw "Rust crate license generation failed."
}

$Sources = [ordered]@{
    "FastPad.exe"            = Join-Path $TargetDirectory "$Target\$BuildProfile\fastpad.exe"
    "Scintilla.dll"          = Join-Path $NativeOutput "Scintilla.dll"
    "Lexilla.dll"            = Join-Path $NativeOutput "Lexilla.dll"
    "README.md"              = Join-Path $RepositoryRoot "README.md"
    "LICENSE"                = Join-Path $RepositoryRoot "LICENSE"
    "LICENSES.md"            = Join-Path $RepositoryRoot "LICENSES.md"
    "licenses\Scintilla.txt" = Join-Path $RepositoryRoot "licenses\Scintilla.txt"
    "licenses\Lexilla.txt"   = Join-Path $RepositoryRoot "licenses\Lexilla.txt"
    "licenses\rust-crates.txt" = $RustCrateLicenses
}
if ((@($Sources.Keys | Sort-Object) -join "|") -ne (@($PackageFiles | Sort-Object) -join "|")) {
    throw "Package sources do not match the package layout."
}

$exeBytes = [System.IO.File]::ReadAllBytes($Sources["FastPad.exe"])
$exeAnsi = [System.Text.Encoding]::Latin1.GetString($exeBytes)
$exeWide = [System.Text.Encoding]::Unicode.GetString($exeBytes)
foreach ($buildPath in @($RepositoryRoot, $CargoHome, $HOME)) {
    foreach ($spelling in @($buildPath, $buildPath.Replace("\", "/"), $buildPath.Replace("\", "\\"))) {
        if ($exeAnsi.IndexOf($spelling, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -or
            $exeWide.IndexOf($spelling, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
            throw "FastPad.exe embeds the build-machine path '$spelling'."
        }
    }
}

if (Test-Path -LiteralPath $StageRoot) {
    Remove-Item -LiteralPath $StageRoot -Recurse -Force
}
New-Item -ItemType Directory -Force -Path (Join-Path $StageRoot "licenses") | Out-Null
foreach ($entry in $Sources.GetEnumerator()) {
    if (-not (Test-Path -LiteralPath $entry.Value -PathType Leaf)) {
        throw "Package input '$($entry.Value)' is missing."
    }
    Copy-Item -LiteralPath $entry.Value -Destination (Join-Path $StageRoot $entry.Key) -Force
}

$Dumpbin = Find-MsvcTool -Name "dumpbin.exe"
foreach ($binary in $PackageBinaries) {
    Assert-Amd64Image -Path (Join-Path $StageRoot $binary)
    Assert-NoDynamicCrtImports -Dumpbin $Dumpbin -Path (Join-Path $StageRoot $binary)
}

$Signed = Test-CodeSigningConfigured
if ($Signed) {
    Invoke-CodeSigning -Paths @($PackageBinaries | ForEach-Object { Join-Path $StageRoot $_ })
}
else {
    Write-Warning "No code signing is configured (see tools/signing.ps1); the package binaries are unsigned."
}

$hashLines = foreach ($relative in $PackageFiles) {
    $staged = Join-Path $StageRoot $relative
    $stagedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $staged).Hash.ToLowerInvariant()
    if (-not ($PackageBinaries -contains $relative) -or -not $Signed) {
        $sourceHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $Sources[$relative]).Hash.ToLowerInvariant()
        if ($stagedHash -ne $sourceHash) {
            throw "Staged '$relative' does not match its source hash."
        }
    }
    "$stagedHash  $($relative.Replace('\', '/'))"
}

if (Test-Path -LiteralPath $ZipPath) {
    Remove-Item -LiteralPath $ZipPath -Force
}
Compress-Archive -Path (Join-Path $StageRoot "*") -DestinationPath $ZipPath -CompressionLevel Optimal
$zipHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $ZipPath).Hash.ToLowerInvariant()
Set-Content -LiteralPath $HashPath -Value (@($hashLines) + "$zipHash  $PackageName.zip") -Encoding utf8NoBOM

Write-Output "Package: $ZipPath"
Write-Output "SHA-256: $zipHash"
