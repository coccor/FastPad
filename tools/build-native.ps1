[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepositoryRoot = Split-Path -Parent $PSScriptRoot
$NativeRoot = Join-Path $RepositoryRoot "native"
$SourceRoot = Join-Path $NativeRoot "src"
$OutputRoot = Join-Path $NativeRoot "out\x64"
$LicensesRoot = Join-Path $RepositoryRoot "licenses"

& (Join-Path $PSScriptRoot "fetch-native.ps1")
if (-not $?) {
    throw "Native dependency fetch failed"
}

foreach ($command in @("nmake.exe", "dumpbin.exe")) {
    if ($null -eq (Get-Command $command -ErrorAction SilentlyContinue)) {
        throw "Required MSVC tool '$command' was not found. Run this script from an x64 Native Tools Command Prompt."
    }
}

Push-Location (Join-Path $SourceRoot "scintilla\win32")
try {
    & nmake.exe /nologo -f scintilla.mak
    if ($LASTEXITCODE -ne 0) { throw "Scintilla build failed" }
}
finally {
    Pop-Location
}

Push-Location (Join-Path $SourceRoot "lexilla\src")
try {
    & nmake.exe /nologo -f lexilla.mak
    if ($LASTEXITCODE -ne 0) { throw "Lexilla build failed" }
}
finally {
    Pop-Location
}

$ScintillaDll = Join-Path $SourceRoot "scintilla\bin\Scintilla.dll"
$LexillaDll = Join-Path $SourceRoot "lexilla\bin\lexilla.dll"
foreach ($dll in @($ScintillaDll, $LexillaDll)) {
    if (-not (Test-Path -LiteralPath $dll -PathType Leaf)) {
        throw "Expected release DLL '$dll' was not produced."
    }
}

New-Item -ItemType Directory -Force -Path $OutputRoot, $LicensesRoot | Out-Null
Copy-Item -LiteralPath $ScintillaDll -Destination (Join-Path $OutputRoot "Scintilla.dll") -Force
Copy-Item -LiteralPath $LexillaDll -Destination (Join-Path $OutputRoot "Lexilla.dll") -Force

Copy-Item -LiteralPath (Join-Path $SourceRoot "scintilla\License.txt") -Destination (Join-Path $LicensesRoot "Scintilla.txt") -Force
Copy-Item -LiteralPath (Join-Path $SourceRoot "lexilla\License.txt") -Destination (Join-Path $LicensesRoot "Lexilla.txt") -Force

foreach ($dll in @(
    (Join-Path $OutputRoot "Scintilla.dll"),
    (Join-Path $OutputRoot "Lexilla.dll")
)) {
    $headers = (& dumpbin.exe /headers $dll) -join [Environment]::NewLine
    if ($LASTEXITCODE -ne 0) {
        throw "dumpbin failed for '$dll'."
    }
    if ($headers -notmatch "machine \(x64\)") {
        throw "'$dll' is not an x64 DLL."
    }
}
