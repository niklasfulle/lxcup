param(
    [ValidateRange(80, 90)]
    [int]$Minimum = 80
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$oldDatabaseTestUrl = $env:DATABASE_TEST_URL

try {
    if ([string]::IsNullOrWhiteSpace($env:DATABASE_TEST_URL)) {
        $dotEnv = Join-Path $root ".env"
        if (Test-Path -LiteralPath $dotEnv) {
            foreach ($line in Get-Content -LiteralPath $dotEnv) {
                if ($line -match '^\s*DATABASE_TEST_URL\s*=\s*(.*)\s*$') {
                    $value = $matches[1].Trim().Trim('"')
                    if (-not [string]::IsNullOrWhiteSpace($value)) {
                        $env:DATABASE_TEST_URL = $value
                    }
                    break
                }
            }
        }
    }

    Write-Host "Rust-Coverage (Minimum: $Minimum%)"
    cargo llvm-cov test --workspace --ignore-filename-regex "\\tests?\.rs$" --fail-under-lines $Minimum --fail-under-functions $Minimum
    if ($LASTEXITCODE -ne 0) {
        throw "Rust-Coverage liegt unter der geforderten Schwelle von $Minimum%."
    }

    $node = Join-Path $PSScriptRoot "..\.cache\node.exe"
    if (-not (Test-Path $node)) {
        $node = "node"
    }

    Push-Location (Join-Path $root "frontend")
    try {
        & $node "node_modules/vitest/vitest.mjs" run --coverage
        if ($LASTEXITCODE -ne 0) {
            throw "Frontend-Coverage liegt unter der geforderten Schwelle von 80%."
        }
    }
    finally {
        Pop-Location
    }
}
finally {
    if ($null -eq $oldDatabaseTestUrl) {
        Remove-Item Env:DATABASE_TEST_URL -ErrorAction SilentlyContinue
    }
    else {
        $env:DATABASE_TEST_URL = $oldDatabaseTestUrl
    }
}
