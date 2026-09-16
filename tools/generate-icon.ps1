[CmdletBinding()]
param(
    [string]$Svg = (Join-Path $PSScriptRoot "..\assets\fastpad-icon.svg"),
    [string]$Output = (Join-Path $PSScriptRoot "..\assets\fastpad.ico"),
    [int[]]$Sizes = @(16, 20, 24, 32, 40, 48, 64, 256)
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.Drawing

function Find-Edge {
    $candidates = @(
        (Join-Path ${env:ProgramFiles(x86)} "Microsoft\Edge\Application\msedge.exe"),
        (Join-Path $env:ProgramFiles "Microsoft\Edge\Application\msedge.exe")
    ) | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf }
    if (@($candidates).Count -eq 0) {
        throw "msedge.exe was not found; it is used to render the SVG."
    }
    return @($candidates)[0]
}

$svgPath = (Resolve-Path -LiteralPath $Svg).Path
$outputPath = [System.IO.Path]::GetFullPath($Output)
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("fastpad-icon-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $work | Out-Null

try {
    # Lay every size out in one row so a single headless render covers them all.
    $gap = 8
    $width = ($Sizes | Measure-Object -Sum).Sum + $gap * ($Sizes.Count + 1)
    $height = ($Sizes | Measure-Object -Maximum).Maximum + 2 * $gap
    $svgUri = ([System.Uri]$svgPath).AbsoluteUri
    $x = $gap
    $images = foreach ($size in $Sizes) {
        "<img src=`"$svgUri`" style=`"position:absolute;left:${x}px;top:${gap}px;width:${size}px;height:${size}px`">"
        $x += $size + $gap
    }
    $html = "<!doctype html><html><body style=`"margin:0;background:transparent`">$($images -join '')</body></html>"
    $htmlPath = Join-Path $work "icon.html"
    Set-Content -LiteralPath $htmlPath -Value $html -Encoding utf8

    $sheetPath = Join-Path $work "sheet.png"
    $edge = Find-Edge
    $arguments = @(
        "--headless=new", "--disable-gpu", "--hide-scrollbars", "--force-device-scale-factor=1",
        "--default-background-color=00000000", "--user-data-dir=$(Join-Path $work 'edge')",
        "--window-size=$width,$height", "--screenshot=$sheetPath", ([System.Uri]$htmlPath).AbsoluteUri
    )
    Start-Process -FilePath $edge -ArgumentList $arguments -Wait -WindowStyle Hidden
    if (-not (Test-Path -LiteralPath $sheetPath -PathType Leaf)) {
        throw "Edge did not produce a screenshot of the icon sheet."
    }

    $sheet = [System.Drawing.Bitmap]::FromFile($sheetPath)
    $entries = @()
    try {
        $x = $gap
        foreach ($size in $Sizes) {
            $frame = $sheet.Clone([System.Drawing.Rectangle]::new($x, $gap, $size, $size), [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
            try {
                $stream = [System.IO.MemoryStream]::new()
                $frame.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
                $entries += [pscustomobject]@{ Size = $size; Png = $stream.ToArray() }
            } finally {
                $frame.Dispose()
            }
            $x += $size + $gap
        }
    } finally {
        $sheet.Dispose()
    }

    # ICO container with PNG-compressed frames (supported since Windows Vista).
    $file = [System.IO.MemoryStream]::new()
    $writer = [System.IO.BinaryWriter]::new($file)
    $writer.Write([uint16]0)
    $writer.Write([uint16]1)
    $writer.Write([uint16]$entries.Count)
    $offset = 6 + 16 * $entries.Count
    foreach ($entry in $entries) {
        $dimension = if ($entry.Size -ge 256) { 0 } else { $entry.Size }
        $writer.Write([byte]$dimension)
        $writer.Write([byte]$dimension)
        $writer.Write([byte]0)
        $writer.Write([byte]0)
        $writer.Write([uint16]1)
        $writer.Write([uint16]32)
        $writer.Write([uint32]$entry.Png.Length)
        $writer.Write([uint32]$offset)
        $offset += $entry.Png.Length
    }
    foreach ($entry in $entries) {
        $writer.Write($entry.Png)
    }
    $writer.Flush()
    [System.IO.File]::WriteAllBytes($outputPath, $file.ToArray())
    Write-Output "Wrote $outputPath ($($entries.Count) sizes: $($Sizes -join ', '))"
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
