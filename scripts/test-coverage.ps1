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
    $rustReport = Join-Path $root "target\coverage-rust.json"
    cargo llvm-cov test --workspace --ignore-filename-regex "\\tests?\.rs$" --fail-under-lines $Minimum --fail-under-functions $Minimum --json --output-path $rustReport
    if ($LASTEXITCODE -ne 0) {
        throw "Rust-Coverage liegt unter der geforderten Schwelle von $Minimum%."
    }

    $rustCoverage = Get-Content -LiteralPath $rustReport -Raw | ConvertFrom-Json
    $uncoveredRustFiles = @(
        foreach ($suite in $rustCoverage.data) {
            foreach ($file in $suite.files) {
                if (
                    $file.filename -match '\.rs$' -and
                    $file.filename -match '[\\/]crates[\\/]' -and
                    $file.filename -notmatch '(?i)(^|[\\/])(tests?|benches|examples)([\\/]|\.rs$)|(^|[\\/])[^\\/]*_(tests?|specs?)\.rs$' -and
                    $file.summary.lines.percent -lt 80
                ) {
                    "{0}: {1:N2}%" -f $file.filename, $file.summary.lines.percent
                }
            }
        }
    )
    if ($uncoveredRustFiles.Count -gt 0) {
        $uncoveredRustFiles | ForEach-Object { Write-Error $_ }
        throw "Jede Rust-Produktionsdatei muss mindestens 80% Zeilenabdeckung erreichen."
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

        $frontendReport = Join-Path (Get-Location) "coverage\lcov.info"
        $lcov = Get-Content -LiteralPath $frontendReport -Raw
        $uncoveredFrontendFiles = @(
            foreach ($record in ($lcov -split '(?m)^end_of_record\s*$')) {
                $source = [regex]::Match($record, '(?m)^SF:(.+)$')
                $lines = [regex]::Matches($record, '(?m)^DA:\d+,(\d+)')
                if (-not $source.Success -or $lines.Count -eq 0) {
                    continue
                }
                $coveredLines = @($lines | Where-Object { [int]$_.Groups[1].Value -gt 0 }).Count
                $lineCoverage = 100 * $coveredLines / $lines.Count
                if ($lineCoverage -lt 80) {
                    "{0}: {1:N2}%" -f $source.Groups[1].Value, $lineCoverage
                }
            }
        )
        if ($uncoveredFrontendFiles.Count -gt 0) {
            $uncoveredFrontendFiles | ForEach-Object { Write-Error $_ }
            throw "Jede Frontend-Produktionsdatei muss mindestens 80% Zeilenabdeckung erreichen."
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
