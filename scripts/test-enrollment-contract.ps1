$ErrorActionPreference = "Stop"

Push-Location (Join-Path $PSScriptRoot "..")
try {
    cargo test -p lxcup-server enrollment_ -- --nocapture
    if ($LASTEXITCODE -ne 0) { throw "Enrollment-API-Tests fehlgeschlagen." }

    & .\scripts\test-frontend.ps1
    if ($LASTEXITCODE -ne 0) { throw "Frontend-Enrollment-Tests fehlgeschlagen." }

    Write-Host "Enrollment-Vertrag lokal erfolgreich geprüft."
} finally {
    Pop-Location
}
