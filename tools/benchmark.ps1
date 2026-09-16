param(
    [ValidateRange(1, [int]::MaxValue)]
    [int]$Runs = 100,

    [ValidateRange(0, [int]::MaxValue)]
    [int]$Warmup = 10,

    [string]$Output = "benchmarks/latest.jsonl",

    [switch]$EnforceReference
)

$ErrorActionPreference = "Stop"
$RepositoryRoot = Split-Path -Parent $PSScriptRoot
Push-Location $RepositoryRoot
try {
    cargo build --release --bin fastpad
    if ($LASTEXITCODE -ne 0) { throw "FastPad release build failed" }

    $BenchmarkArguments = @(
        "run", "--release", "--bin", "fastpad-bench", "--",
        "--runs", $Runs,
        "--warmup", $Warmup,
        "--output", $Output
    )
    if ($EnforceReference) { $BenchmarkArguments += "--enforce-reference" }
    & cargo @BenchmarkArguments
    if ($LASTEXITCODE -ne 0) { throw "FastPad startup benchmark failed" }
}
finally {
    Pop-Location
}
