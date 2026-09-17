# Makes ISCC.exe (Inno Setup 6) available on a CI runner. Local builds can use:
#   winget install JRSoftware.InnoSetup --scope user

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$known = @(
    (Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6\ISCC.exe"),
    (Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"),
    (Join-Path $env:ProgramFiles "Inno Setup 6\ISCC.exe")
)
if ((Get-Command ISCC.exe -ErrorAction SilentlyContinue) -or ($known | Where-Object { Test-Path -LiteralPath $_ })) {
    Write-Output "Inno Setup is already installed."
    return
}
& choco install innosetup --no-progress -y
if ($LASTEXITCODE -ne 0) {
    throw "choco install innosetup failed."
}
