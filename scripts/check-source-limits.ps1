$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$extensions = @(
    ".rs", ".ts", ".tsx", ".js", ".jsx", ".css", ".scss", ".ps1",
    ".sh", ".yml", ".yaml", ".sql", ".toml", ".html", ".conf"
)
$generatedOrTestPath = '[\\/](\.git|node_modules|target|coverage|dist|build|\.next|fixtures?|testdata|tests?|benches|examples)[\\/]'
$testFileName = '(^|[.-])(test|tests|spec|specs)([.-]|$)|(_tests?|_specs?)\.'
$generatedFileName = '^(Cargo|pnpm-lock|package-lock|yarn\.lock)'
$violations = @(
    Get-ChildItem -LiteralPath $root -Recurse -File |
        Where-Object {
            $extensions -contains $_.Extension.ToLowerInvariant() -and
            $_.FullName -notmatch $generatedOrTestPath -and
            $_.Name -notmatch $testFileName -and
            $_.Name -notmatch $generatedFileName
        } |
        ForEach-Object {
            $lineCount = (Get-Content -LiteralPath $_.FullName | Measure-Object -Line).Lines
            if ($lineCount -gt 800) {
                [pscustomobject]@{ Path = $_.FullName; Lines = $lineCount }
            }
        }
)

if ($violations.Count -gt 0) {
    $violations | Format-Table -AutoSize | Out-String | Write-Error
    throw "Produktionsdateien dürfen höchstens 800 Zeilen enthalten."
}

Write-Host "Quelltext-Grenze erfüllt: alle geprüften Produktionsdateien liegen bei höchstens 800 Zeilen."
