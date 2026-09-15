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
    "licenses\Lexilla.txt"
)
$PackageBinaries = @("FastPad.exe", "Scintilla.dll", "Lexilla.dll")

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
