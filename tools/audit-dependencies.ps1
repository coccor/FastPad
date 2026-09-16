[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepositoryRoot = Split-Path -Parent $PSScriptRoot
$AllowedRoots = @(
    [pscustomobject]@{ Name = "windows-sys"; Version = "0.61.2" },
    [pscustomobject]@{ Name = "serde_json"; Version = "1.0.151" },
    [pscustomobject]@{ Name = "pulldown-cmark"; Version = "0.13.4" },
    [pscustomobject]@{ Name = "windows"; Version = "0.62.2" },
    [pscustomobject]@{ Name = "windows-numerics"; Version = "0.3.1" }
)
# The `windows` crate is allowed only as the pinned direct dependency, with exactly these features.
# Win32/Win32_Graphics/Win32_Graphics_Dxgi/Win32_System are namespace features that cargo metadata
# reports as implied by the leaf features below; they add no API surface of their own.
$AllowedWindowsFeatures = @(
    "Win32", "Win32_Foundation", "Win32_Graphics", "Win32_Graphics_Direct2D",
    "Win32_Graphics_Direct2D_Common", "Win32_Graphics_DirectWrite", "Win32_Graphics_Dxgi",
    "Win32_Graphics_Dxgi_Common", "Win32_Graphics_Imaging", "Win32_System", "Win32_System_Com"
)
$RegistrySource = "registry+https://github.com/rust-lang/crates.io-index"

# Crate-name patterns for HTTP, TLS, sockets, updaters, telemetry, async runtimes, and UI frameworks.
$RejectedPatterns = @(
    '^(hyper|hyper-.*|reqwest|ureq|curl|curl-sys|isahc|surf|attohttpc|http|http-body|httparse|h2|h3|minreq)$',
    '^(rustls|rustls-.*|openssl|openssl-sys|native-tls|tokio-rustls|tokio-native-tls|schannel|security-framework|webpki|webpki-roots|boring|boring-sys)$',
    '^(socket2|mio|tungstenite|tokio-tungstenite|websocket|ws|quinn|quinn-.*|trust-dns-.*|hickory-.*|libp2p.*|zmq)$',
    '^(self_update|self-replace|update-informer|velopack|tauri-plugin-updater|squirrel.*)$',
    '^(sentry|sentry-.*|opentelemetry|opentelemetry-.*|posthog.*|segment|datadog.*|rudderstack|telemetry.*|metrics-exporter-.*)$',
    '^(tokio|tokio-.*|async-std|smol|async-executor|async-io|async-global-executor|futures|futures-executor|actix|actix-.*|glommio|monoio)$',
    '^(winit|egui|eframe|iced|iced_.*|gtk|gtk4|relm.*|tauri|tauri-.*|wry|tao|druid|slint|slint-.*|fltk|fltk-.*|native-windows-gui|winsafe|webview2|webview2-com|dioxus.*|makepad.*|floem|xilem|vizia|imgui.*|sdl2|glutin)$'
)

Push-Location $RepositoryRoot
try {
    $json = & cargo metadata --locked --format-version 1
    if ($LASTEXITCODE -ne 0) {
        throw "cargo metadata --locked failed; Cargo.lock may be out of date."
    }
}
finally {
    Pop-Location
}

$metadata = $json | ConvertFrom-Json
$packagesById = @{}
foreach ($package in $metadata.packages) {
    $packagesById[$package.id] = $package
}
$nodesById = @{}
foreach ($node in $metadata.resolve.nodes) {
    $nodesById[$node.id] = $node
}

$rootId = $metadata.resolve.root
if ($null -eq $rootId -or $packagesById[$rootId].name -ne "fastpad") {
    throw "cargo metadata did not resolve the fastpad package as its root."
}

$directDependencies = @($nodesById[$rootId].deps | ForEach-Object { $packagesById[$_.pkg] })
$failures = [System.Collections.Generic.List[string]]::new()
foreach ($dependency in $directDependencies) {
    $allowed = $AllowedRoots | Where-Object { $_.Name -eq $dependency.name -and $_.Version -eq $dependency.version }
    if ($null -eq $allowed) {
        $failures.Add("direct dependency $($dependency.name) $($dependency.version) is not allowed")
    }
}
foreach ($root in $AllowedRoots) {
    if (-not ($directDependencies | Where-Object { $_.name -eq $root.Name -and $_.version -eq $root.Version })) {
        $failures.Add("required direct dependency $($root.Name) $($root.Version) is missing")
    }
}

$closure = [System.Collections.Generic.HashSet[string]]::new()
$pending = [System.Collections.Generic.Stack[string]]::new()
foreach ($dependency in $directDependencies) {
    $pending.Push($dependency.id)
}
while ($pending.Count -gt 0) {
    $id = $pending.Pop()
    if (-not $closure.Add($id)) {
        continue
    }
    foreach ($edge in $nodesById[$id].deps) {
        $pending.Push($edge.pkg)
    }
}

foreach ($node in $metadata.resolve.nodes) {
    if ($node.id -eq $rootId) {
        continue
    }
    $package = $packagesById[$node.id]
    $label = "$($package.name) $($package.version)"
    if (-not $closure.Contains($node.id)) {
        $failures.Add("$label is outside the allowed dependency closure")
    }
    if ($package.source -ne $RegistrySource) {
        $failures.Add("$label does not come from crates.io ($($package.source))")
    }
    if ($package.name -eq "windows") {
        $isRoot = $directDependencies | Where-Object { $_.id -eq $node.id }
        if ($null -eq $isRoot) {
            $failures.Add("$label may only appear as FastPad's direct dependency")
        }
        foreach ($feature in $node.features) {
            if ($AllowedWindowsFeatures -notcontains $feature) {
                $failures.Add("$label enables feature $feature, which is not allowed")
            }
        }
    }
    foreach ($pattern in $RejectedPatterns) {
        if ($package.name -match $pattern) {
            $failures.Add("$label is a rejected network, async, telemetry, updater, or UI crate")
        }
    }
}

if ($failures.Count -gt 0) {
    $failures | ForEach-Object { Write-Error -ErrorAction Continue "dependency audit: $_" }
    throw "Dependency audit failed with $($failures.Count) violation(s)."
}

$closure | ForEach-Object { $packagesById[$_] } | Sort-Object name |
    ForEach-Object { Write-Output "allowed: $($_.name) $($_.version) ($($_.license))" }
Write-Output "Dependency audit passed: $($closure.Count) crates in the allowed dependency closure."
