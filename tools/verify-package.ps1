[CmdletBinding()]
param(
    [string]$Package,
    [switch]$RequireSignature
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepositoryRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot "msvc.ps1")
. (Join-Path $PSScriptRoot "package-layout.ps1")
if ([string]::IsNullOrWhiteSpace($Package)) {
    $Package = Join-Path $RepositoryRoot "dist\$PackageName.zip"
}
if (-not (Test-Path -LiteralPath $Package -PathType Leaf)) {
    throw "Package '$Package' does not exist. Run tools/package.ps1 first."
}

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class FastPadSmoke {
    delegate bool EnumProc(IntPtr hwnd, IntPtr lparam);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc proc, IntPtr lparam);
    [DllImport("user32.dll")] static extern bool EnumChildWindows(IntPtr parent, EnumProc proc, IntPtr lparam);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr hwnd, StringBuilder name, int count);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int count);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr SendMessageTimeout(IntPtr hwnd, uint message, IntPtr wparam, IntPtr lparam, uint flags, uint timeout, out IntPtr result);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr SendMessageTimeout(IntPtr hwnd, uint message, IntPtr wparam, StringBuilder lparam, uint flags, uint timeout, out IntPtr result);
    [DllImport("user32.dll")] static extern bool PostMessage(IntPtr hwnd, uint message, IntPtr wparam, IntPtr lparam);

    static string ClassOf(IntPtr hwnd) { var name = new StringBuilder(128); GetClassName(hwnd, name, name.Capacity); return name.ToString(); }

    public static IntPtr FindTopLevel(uint pid, string className) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((hwnd, _) => {
            uint owner; GetWindowThreadProcessId(hwnd, out owner);
            if (owner == pid && ClassOf(hwnd) == className) { found = hwnd; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }

    public static IntPtr FindChild(IntPtr parent, string className) {
        IntPtr found = IntPtr.Zero;
        EnumChildWindows(parent, (hwnd, _) => {
            if (ClassOf(hwnd) == className) { found = hwnd; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }

    public static bool SendChar(IntPtr hwnd, char character) {
        IntPtr result;
        return SendMessageTimeout(hwnd, 0x0102, (IntPtr)character, (IntPtr)1, 0x2, 5000, out result) != IntPtr.Zero;
    }

    public static string GetText(IntPtr hwnd) {
        IntPtr length;
        if (SendMessageTimeout(hwnd, 0x000E, IntPtr.Zero, IntPtr.Zero, 0x2, 5000, out length) == IntPtr.Zero) return null;
        var text = new StringBuilder(length.ToInt32() + 1);
        IntPtr ignored;
        if (SendMessageTimeout(hwnd, 0x000D, (IntPtr)text.Capacity, text, 0x2, 5000, out ignored) == IntPtr.Zero) return null;
        return text.ToString();
    }

    public static void Close(IntPtr hwnd) { PostMessage(hwnd, 0x0010, IntPtr.Zero, IntPtr.Zero); }

    public static bool DismissDiscardPrompt(uint pid) {
        IntPtr dialog = FindTopLevel(pid, "#32770");
        if (dialog == IntPtr.Zero) return false;
        IntPtr button = IntPtr.Zero;
        EnumChildWindows(dialog, (hwnd, _) => {
            var text = new StringBuilder(32); GetWindowText(hwnd, text, text.Capacity);
            var caption = text.ToString().TrimStart('&');
            if (caption.Equals("No", StringComparison.OrdinalIgnoreCase)) { button = hwnd; return false; }
            return true;
        }, IntPtr.Zero);
        if (button == IntPtr.Zero) return false;
        IntPtr result;
        SendMessageTimeout(button, 0x00F5, IntPtr.Zero, IntPtr.Zero, 0x2, 5000, out result);
        return true;
    }
}
"@

function Wait-Until {
    param(
        [Parameter(Mandatory = $true)] [string]$What,
        [Parameter(Mandatory = $true)] [scriptblock]$Condition,
        [int]$TimeoutSeconds = 15
    )
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ($true) {
        $value = & $Condition
        if ($value) { return $value }
        if ([DateTime]::UtcNow -ge $deadline) { throw "Timed out waiting for $What." }
        Start-Sleep -Milliseconds 25
    }
}

$VerifyRoot = Join-Path ([System.IO.Path]::GetTempPath()) "fastpad-verify-$([guid]::NewGuid().ToString('N'))"
$PackageRoot = Join-Path $VerifyRoot "package"
$WorkRoot = Join-Path $VerifyRoot "unrelated-working-directory"
$LocalAppData = Join-Path $VerifyRoot "local-app-data"
$process = $null
try {
    New-Item -ItemType Directory -Force -Path $PackageRoot, $WorkRoot, $LocalAppData | Out-Null
    Expand-Archive -LiteralPath $Package -DestinationPath $PackageRoot

    $actual = @(Get-ChildItem -LiteralPath $PackageRoot -Recurse -File |
        ForEach-Object { [System.IO.Path]::GetRelativePath($PackageRoot, $_.FullName) } |
        Sort-Object)
    $expected = @($PackageFiles | Sort-Object)
    $missing = @($expected | Where-Object { $actual -notcontains $_ })
    $extra = @($actual | Where-Object { $expected -notcontains $_ })
    if ($missing.Count -gt 0 -or $extra.Count -gt 0) {
        throw "Package contents are wrong. Missing: [$($missing -join ', ')]; extra: [$($extra -join ', ')]."
    }
    Write-Output "Package contains exactly: $($expected -join ', ')"

    $dumpbin = Find-MsvcTool -Name "dumpbin.exe"
    foreach ($binary in $PackageBinaries) {
        Assert-Amd64Image -Path (Join-Path $PackageRoot $binary)
        Assert-NoDynamicCrtImports -Dumpbin $dumpbin -Path (Join-Path $PackageRoot $binary)
    }
    Write-Output "No packaged binary imports the dynamic C runtime."

    if ($RequireSignature) {
        foreach ($binary in $PackageBinaries) {
            $signature = Get-AuthenticodeSignature -LiteralPath (Join-Path $PackageRoot $binary)
            if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
                throw "$binary does not have a valid Authenticode signature ($($signature.Status): $($signature.StatusMessage))."
            }
        }
        Write-Output "Authenticode signatures are valid."
    }

    $jsonFixture = Join-Path $WorkRoot "smoke.json"
    Set-Content -LiteralPath $jsonFixture -Value '{"smoke": true}' -NoNewline -Encoding utf8NoBOM

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new((Join-Path $PackageRoot "FastPad.exe"))
    $startInfo.UseShellExecute = $false
    $startInfo.WorkingDirectory = $WorkRoot
    $startInfo.ArgumentList.Add("--new-window")
    $startInfo.ArgumentList.Add("smoke.json")
    $startInfo.Environment["LOCALAPPDATA"] = $LocalAppData
    $process = [System.Diagnostics.Process]::Start($startInfo)
    $processId = [uint32]$process.Id

    $mainWindow = Wait-Until "the FastPad main window" {
        if ($process.HasExited) { throw "FastPad exited early with code $($process.ExitCode)." }
        $hwnd = [FastPadSmoke]::FindTopLevel($processId, "FastPadMainWindow")
        if ($hwnd -ne [IntPtr]::Zero) { $hwnd }
    }
    $editor = Wait-Until "the Scintilla editor" {
        $hwnd = [FastPadSmoke]::FindChild($mainWindow, "Scintilla")
        if ($hwnd -ne [IntPtr]::Zero) { $hwnd }
    }
    if (-not [FastPadSmoke]::SendChar($editor, [char]'x')) {
        throw "FastPad did not accept keyboard input."
    }
    Wait-Until "the launch file to open after first input" {
        [FastPadSmoke]::GetText($editor) -eq '{"smoke": true}'
    } | Out-Null

    $packagePrefix = [System.IO.Path]::TrimEndingDirectorySeparator($PackageRoot) + "\"
    $system32Prefix = (Join-Path $env:SystemRoot "System32") + "\"
    # Common Controls v6 is served from the OS side-by-side store.
    $winSxSPrefix = (Join-Path $env:SystemRoot "WinSxS") + "\"
    $systemPrefixes = @($system32Prefix, $winSxSPrefix)
    $modules = Wait-Until "Lexilla to load from the package" {
        $process.Refresh()
        $loaded = @($process.Modules | ForEach-Object { $_.FileName })
        if ($loaded | Where-Object { $_ -ieq (Join-Path $PackageRoot "Lexilla.dll") }) { , $loaded }
    }
    foreach ($required in $PackageBinaries) {
        $path = Join-Path $PackageRoot $required
        if (-not ($modules | Where-Object { $_ -ieq $path })) {
            throw "$required was not loaded from the package directory."
        }
    }
    $outside = @($modules | Where-Object {
            $module = $_
            -not $module.StartsWith($packagePrefix, [System.StringComparison]::OrdinalIgnoreCase) -and
            -not ($systemPrefixes | Where-Object { $module.StartsWith($_, [System.StringComparison]::OrdinalIgnoreCase) })
        })
    if ($outside.Count -gt 0) {
        throw "FastPad loaded DLLs from outside the package or System32: $($outside -join '; ')"
    }
    Write-Output "Loaded $($modules.Count) modules, all from the package, System32, or WinSxS."

    [FastPadSmoke]::Close($mainWindow)
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while (-not $process.WaitForExit(50)) {
        [FastPadSmoke]::DismissDiscardPrompt($processId) | Out-Null
        if ([DateTime]::UtcNow -ge $deadline) { throw "FastPad did not exit after WM_CLOSE." }
    }
    if ($process.ExitCode -ne 0) {
        throw "FastPad exited with code $($process.ExitCode)."
    }
    Write-Output "Smoke test passed: launched from '$WorkRoot', accepted input, opened JSON, exited cleanly."
}
finally {
    if ($null -ne $process -and -not $process.HasExited) {
        $process.Kill()
        $process.WaitForExit(5000) | Out-Null
    }
    if (Test-Path -LiteralPath $VerifyRoot) {
        Remove-Item -LiteralPath $VerifyRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
