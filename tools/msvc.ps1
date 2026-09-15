[CmdletBinding()]
param(
    [string]$Find
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-VsInstallationPath {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) {
        throw "vswhere.exe was not found at '$vswhere'. Install Visual Studio 2022 with the C++ x64 build tools."
    }
    $path = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($path)) {
        throw "vswhere found no Visual Studio installation with the C++ x64 build tools."
    }
    return ($path | Select-Object -First 1).Trim()
}

function Find-MsvcTool {
    param([Parameter(Mandatory = $true)] [string]$Name)

    $installation = Get-VsInstallationPath
    $toolsRoot = Join-Path $installation "VC\Tools\MSVC"
    $candidates = @(Get-ChildItem -LiteralPath $toolsRoot -Directory -ErrorAction SilentlyContinue |
        Sort-Object { [version]$_.Name } -Descending |
        ForEach-Object { Join-Path $_.FullName "bin\Hostx64\x64\$Name" } |
        Where-Object { Test-Path -LiteralPath $_ -PathType Leaf })
    if ($candidates.Count -eq 0) {
        throw "MSVC tool '$Name' was not found under '$toolsRoot'."
    }
    return $candidates[0]
}

function Enter-MsvcEnvironment {
    if ($null -ne (Get-Command nmake.exe -ErrorAction SilentlyContinue) -and
        $null -ne (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
        return
    }
    $installation = Get-VsInstallationPath
    Import-Module (Join-Path $installation "Common7\Tools\Microsoft.VisualStudio.DevShell.dll")
    Enter-VsDevShell -VsInstallPath $installation -SkipAutomaticLocation -DevCmdArguments "-arch=x64 -host_arch=x64" | Out-Null
}

function Find-SignTool {
    $kitsRoot = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
    $candidates = @(Get-ChildItem -LiteralPath $kitsRoot -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '^\d+\.\d+\.\d+\.\d+$' } |
        Sort-Object { [version]$_.Name } -Descending |
        ForEach-Object { Join-Path $_.FullName "x64\signtool.exe" } |
        Where-Object { Test-Path -LiteralPath $_ -PathType Leaf })
    if ($candidates.Count -eq 0) {
        throw "signtool.exe was not found under '$kitsRoot'. Install the Windows 10/11 SDK."
    }
    return $candidates[0]
}

if (-not [string]::IsNullOrWhiteSpace($Find)) {
    Write-Output (Find-MsvcTool -Name $Find)
}
