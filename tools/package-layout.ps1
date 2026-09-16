Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$PackageName = "FastPad-0.1.0-windows-x64"
$PackageFiles = @(
    "FastPad.exe",
    "Scintilla.dll",
    "Lexilla.dll",
    "README.md",
    "LICENSES.md",
    "licenses\Scintilla.txt",
    "licenses\Lexilla.txt",
    "licenses\rust-crates.txt"
)
$PackageBinaries = @("FastPad.exe", "Scintilla.dll", "Lexilla.dll")
# The Visual C++ runtime redistributable is not part of a clean Windows install.
$DynamicCrtImportPattern = '^(vcruntime.*|msvcp.*|ucrtbase.*|api-ms-win-crt-.*)\.dll$'
# FastPad makes no network requests, so the shipped executable must not link a network stack.
$NetworkImportPattern = '^(ws2_32|winhttp|wininet|urlmon)\.dll$'

function Get-PeMachine {
    param([Parameter(Mandatory = $true)] [string]$Path)

    $stream = [System.IO.File]::OpenRead($Path)
    try {
        $reader = [System.IO.BinaryReader]::new($stream)
        if ($reader.ReadUInt16() -ne 0x5A4D) {
            throw "'$Path' is not a PE image."
        }
        $stream.Position = 0x3C
        $stream.Position = $reader.ReadInt32()
        if ($reader.ReadUInt32() -ne 0x00004550) {
            throw "'$Path' has no PE signature."
        }
        return $reader.ReadUInt16()
    }
    finally {
        $stream.Dispose()
    }
}

function Assert-Amd64Image {
    param([Parameter(Mandatory = $true)] [string]$Path)

    $machine = Get-PeMachine -Path $Path
    if ($machine -ne 0x8664) {
        throw "'$Path' has PE machine type 0x$($machine.ToString('X4')), expected AMD64 (0x8664)."
    }
}

function Assert-NoNetworkImports {
    param(
        [Parameter(Mandatory = $true)] [string]$Dumpbin,
        [Parameter(Mandatory = $true)] [string]$Path
    )

    $output = & $Dumpbin /nologo /imports $Path
    if ($LASTEXITCODE -ne 0) {
        throw "dumpbin /imports failed for '$Path'."
    }
    $imports = @($output | ForEach-Object { $_.Trim() } | Where-Object { $_ -match '^[^\s]+\.dll$' })
    if ($imports.Count -eq 0) {
        throw "dumpbin /imports for '$Path' listed no DLL imports; import parsing failed."
    }
    $network = @($imports | Where-Object { $_ -imatch $NetworkImportPattern })
    if ($network.Count -gt 0) {
        throw "'$Path' imports network libraries: $($network -join ', ')."
    }
}

function Assert-NoDynamicCrtImports {
    param(
        [Parameter(Mandatory = $true)] [string]$Dumpbin,
        [Parameter(Mandatory = $true)] [string]$Path
    )

    $output = & $Dumpbin /nologo /imports $Path
    if ($LASTEXITCODE -ne 0) {
        throw "dumpbin /imports failed for '$Path'."
    }
    $imports = @($output | ForEach-Object { $_.Trim() } | Where-Object { $_ -match '^[^\s]+\.dll$' })
    if (-not ($imports | Where-Object { $_ -ieq "kernel32.dll" })) {
        throw "dumpbin /imports for '$Path' listed no KERNEL32.dll import; import parsing failed."
    }
    $crt = @($imports | Where-Object { $_ -imatch $DynamicCrtImportPattern })
    if ($crt.Count -gt 0) {
        throw "'$Path' imports the dynamic C runtime: $($crt -join ', ')."
    }
}
