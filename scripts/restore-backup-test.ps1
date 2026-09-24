[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$BackupFile,
    [Parameter(Mandatory = $true)]
    [string]$AgeIdentity,
    [string]$RestoreDirectory = ".\restore-test"
)

$ErrorActionPreference = "Stop"
if (-not (Test-Path -LiteralPath $BackupFile)) { throw "Backup file does not exist." }
if (-not (Get-Command age -ErrorAction SilentlyContinue)) { throw "age is required for restore tests." }
if (-not (Get-Command pg_restore -ErrorAction SilentlyContinue)) { throw "pg_restore is required for restore tests." }
if (-not (Get-Command psql -ErrorAction SilentlyContinue)) { throw "psql is required to verify restored references and audit data." }
$restore = Join-Path (Resolve-Path (New-Item -ItemType Directory -Force -Path $RestoreDirectory)) ("run-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $restore | Out-Null
try {
    age --decrypt --identity $AgeIdentity $BackupFile | tar -xf - -C $restore
    if ($LASTEXITCODE -ne 0) { throw "Backup decryption failed." }
    # Restore only into the explicitly configured isolated test database.
    if ([string]::IsNullOrWhiteSpace($env:DATABASE_TEST_URL)) { throw "DATABASE_TEST_URL must point to an isolated restore database." }
    pg_restore --clean --if-exists --no-owner --dbname=$env:DATABASE_TEST_URL (Join-Path $restore "postgres.dump")
    if ($LASTEXITCODE -ne 0) { throw "PostgreSQL restore failed." }
    $tables = psql --dbname=$env:DATABASE_TEST_URL --tuples-only --no-align --command "SELECT table_name FROM information_schema.tables WHERE table_schema='public' AND table_name IN ('targets','audit_events','package_inventory_snapshots') ORDER BY table_name"
    $tableCount = (($tables -split "\r?\n") | Where-Object { $_ }).Count
    if ($LASTEXITCODE -ne 0 -or $tableCount -lt 3) { throw "Restored database is missing required target, inventory, or audit tables." }
    if (-not (Test-Path (Join-Path $restore "secrets"))) { throw "Encrypted secret-store payload is missing." }
    Write-Host "Isolated restore test succeeded. Secret values were not printed."
}
finally {
    if (Test-Path -LiteralPath $restore) { Remove-Item -LiteralPath $restore -Recurse -Force }
}
