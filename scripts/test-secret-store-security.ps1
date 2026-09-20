[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

Write-Host "Secret-Store-Unit-Tests..."
cargo test -p lxcup-secrets

Write-Host "API-Redaction- und Berechtigungstests..."
cargo test -p lxcup-server --lib secret_api_redacts_values_and_records_lifecycle_audit

Write-Host "Secret-Store-Sicherheitsprüfungen erfolgreich."
