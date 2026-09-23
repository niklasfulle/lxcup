param(
    [ValidateRange(80, 90)]
    [int]$Minimum = 80
)

$ErrorActionPreference = "Stop"

Write-Host "Rust-Coverage (Minimum: $Minimum%)"
cargo llvm-cov test --workspace --ignore-filename-regex "\\tests?\\.rs$" --fail-under-lines $Minimum --fail-under-functions $Minimum
if ($LASTEXITCODE -ne 0) {
    throw "Rust-Coverage liegt unter der geforderten Schwelle von $Minimum%."
}

$node = Join-Path $PSScriptRoot "..\.cache\node.exe"
if (-not (Test-Path $node)) {
    $node = "node"
}

Push-Location (Join-Path $PSScriptRoot "..\frontend")
try {
    & $node "node_modules/vitest/vitest.mjs" run --coverage
    if ($LASTEXITCODE -ne 0) {
        throw "Frontend-Coverage liegt unter der geforderten Schwelle von 80%."
    }
}
finally {
    Pop-Location
}
