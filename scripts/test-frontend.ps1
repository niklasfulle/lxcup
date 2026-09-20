param(
    [switch]$Coverage
)

$ErrorActionPreference = "Stop"
$node = Join-Path $PSScriptRoot "..\.cache\node.exe"
if (-not (Test-Path $node)) {
    $node = "node"
}

Push-Location (Join-Path $PSScriptRoot "..\frontend")
try {
    & $node "node_modules/typescript/bin/tsc" -p tsconfig.json --noEmit
    if ($LASTEXITCODE -ne 0) { throw "Frontend-Typprüfung fehlgeschlagen." }

    $vitestArgs = @("node_modules/vitest/vitest.mjs", "run")
    if ($Coverage) { $vitestArgs += "--coverage" }
    & $node @vitestArgs
    if ($LASTEXITCODE -ne 0) { throw "Frontend-Tests fehlgeschlagen." }

    & $node "node_modules/vite/bin/vite.js" build
    if ($LASTEXITCODE -ne 0) { throw "Frontend-Produktionsbuild fehlgeschlagen." }
} finally {
    Pop-Location
}
