param(
    [string]$DatabaseUrl = $env:DATABASE_URL,
    [string]$BackupDirectory = ".\backups"
)

$ErrorActionPreference = "Stop"
if ([string]::IsNullOrWhiteSpace($DatabaseUrl)) { throw "DATABASE_URL must be configured outside the repository." }
New-Item -ItemType Directory -Force -Path $BackupDirectory | Out-Null
$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$output = Join-Path $BackupDirectory "lxcup-$timestamp.dump"
pg_dump --format=custom --file=$output $DatabaseUrl
Write-Host "PostgreSQL backup created: $output"
