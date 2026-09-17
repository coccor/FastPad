[CmdletBinding()]
param(
    [string]$Svg = (Join-Path $PSScriptRoot "..\assets\fastpad-icon.svg"),
    [string]$Output = (Join-Path $PSScriptRoot "..\assets\fastpad.ico"),
    [int[]]$Sizes = @(16, 20, 24, 32, 40, 48, 64, 256)
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.Drawing

# Frames below 256 px are uncompressed 32-bit DIBs. USER32 decodes a PNG-compressed icon frame through
# WIC, so a PNG frame picked by LoadIconW at startup loads windowscodecs.dll into FastPad (a startup
# cost, and the Markdown preview's zero-startup-cost guard). Only the 256 px frame, read by Explorer
# in its own process, stays PNG to keep the executable small.
function ConvertTo-IconFrame {
    param([Parameter(Mandatory = $true)] [System.Drawing.Bitmap]$Bitmap)

    $size = $Bitmap.Width
    $stream = [System.IO.MemoryStream]::new()
    if ($size -ge 256) {
        $Bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
        return , $stream.ToArray()
    }
    $maskStride = [int][Math]::Floor(($size + 31) / 32) * 4
    $writer = [System.IO.BinaryWriter]::new($stream)
    # BITMAPINFOHEADER: the height covers the XOR (color) and AND (mask) bitmaps together.
    $writer.Write([uint32]40)
    $writer.Write([int32]$size)
    $writer.Write([int32]($size * 2))
    $writer.Write([uint16]1)
    $writer.Write([uint16]32)
    $writer.Write([uint32]0)
    $writer.Write([uint32]($size * $size * 4 + $maskStride * $size))
    $writer.Write([int32]0)
    $writer.Write([int32]0)
    $writer.Write([uint32]0)
    $writer.Write([uint32]0)
    # Bottom-up BGRA rows; the alpha channel carries transparency, so the AND mask stays all zero.
    for ($y = $size - 1; $y -ge 0; $y--) {
        for ($x = 0; $x -lt $size; $x++) {
            $pixel = $Bitmap.GetPixel($x, $y)
            $writer.Write([byte]$pixel.B)
            $writer.Write([byte]$pixel.G)
            $writer.Write([byte]$pixel.R)
            $writer.Write([byte]$pixel.A)
        }
    }
    $writer.Write([byte[]]::new($maskStride * $size))
    $writer.Flush()
    return , $stream.ToArray()
}

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
                $entries += [pscustomobject]@{ Size = $size; Data = (ConvertTo-IconFrame -Bitmap $frame) }
            } finally {
                $frame.Dispose()
            }
            $x += $size + $gap
        }
    } finally {
        $sheet.Dispose()
    }

    # ICO container: DIB frames below 256 px, a PNG-compressed 256 px frame (supported since Vista).
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
        $writer.Write([uint32]$entry.Data.Length)
        $writer.Write([uint32]$offset)
        $offset += $entry.Data.Length
    }
    foreach ($entry in $entries) {
        $writer.Write($entry.Data)
    }
    $writer.Flush()
    [System.IO.File]::WriteAllBytes($outputPath, $file.ToArray())
    Write-Output "Wrote $outputPath ($($entries.Count) sizes: $($Sizes -join ', '))"
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
