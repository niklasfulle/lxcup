[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$BackupFile,
    [Parameter(Mandatory = $true)]
    [string]$AgeIdentity,
    [string]$RestoreDirectory = ".\restore-test",
    [Parameter(Mandatory = $true)]
    [switch]$ConfirmIsolatedDatabase,
    [string]$PostgresToolsContainer
)

$ErrorActionPreference = "Stop"
function Protect-PrivateDirectory([string]$Path) {
    if ($env:OS -eq "Windows_NT") {
        $identity = [Security.Principal.WindowsIdentity]::GetCurrent().Name
        & icacls $Path /inheritance:r /grant:r "${identity}:(OI)(CI)F" /Q | Out-Null
    } else {
        & chmod 700 -- $Path
    }
    if ($LASTEXITCODE -ne 0) { throw "Could not restrict access to temporary restore data." }
}

$previousDatabaseUrl = $env:DATABASE_URL
$previousSecretStoreDir = $env:LXCUP_SECRET_STORE_DIR
if (-not (Test-Path -LiteralPath $BackupFile)) { throw "Backup file does not exist." }
if (-not (Get-Command age -ErrorAction SilentlyContinue)) { throw "age is required for restore tests." }
if ([string]::IsNullOrWhiteSpace($PostgresToolsContainer)) {
    if (-not (Get-Command pg_restore -ErrorAction SilentlyContinue)) { throw "pg_restore is required for restore tests." }
    if (-not (Get-Command psql -ErrorAction SilentlyContinue)) { throw "psql is required to verify restored references and audit data." }
} elseif (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
    throw "docker is required when PostgresToolsContainer is specified."
}
$restore = Join-Path (Resolve-Path (New-Item -ItemType Directory -Force -Path $RestoreDirectory)) ("run-" + [guid]::NewGuid())
try {
    New-Item -ItemType Directory -Force -Path $restore | Out-Null
    Protect-PrivateDirectory $restore
    $archive = Join-Path $restore "backup.tar"
    age --decrypt --identity $AgeIdentity --output $archive $BackupFile
    if ($LASTEXITCODE -ne 0) { throw "Backup decryption failed." }
    tar -xf $archive -C $restore
    if ($LASTEXITCODE -ne 0) { throw "Backup archive extraction failed." }
    # Require explicit confirmation and a test/restore-named database before destructive restore.
    if ([string]::IsNullOrWhiteSpace($env:DATABASE_TEST_URL)) { throw "DATABASE_TEST_URL must point to an isolated restore database." }
    if (-not $ConfirmIsolatedDatabase) { throw "Pass -ConfirmIsolatedDatabase only after verifying DATABASE_TEST_URL is isolated." }
    $databaseUri = [uri]$env:DATABASE_TEST_URL
    $databaseName = $databaseUri.AbsolutePath.TrimStart('/')
    if ($databaseName -notmatch '(?i)(test|restore)') { throw "DATABASE_TEST_URL database name must contain 'test' or 'restore'." }
    $databaseUser = [uri]::UnescapeDataString(($databaseUri.UserInfo -split ':', 2)[0])
    if ([string]::IsNullOrWhiteSpace($databaseUser)) { throw "DATABASE_TEST_URL must include a database user." }
    $containerDumpPath = $null
    if (-not [string]::IsNullOrWhiteSpace($PostgresToolsContainer)) {
        $containerDumpPath = "/tmp/lxcup-restore-$([guid]::NewGuid().ToString('N')).dump"
        docker cp (Join-Path $restore "postgres.dump") "${PostgresToolsContainer}:$containerDumpPath" | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "Could not copy the PostgreSQL dump into the isolated tools container." }
        docker exec $PostgresToolsContainer pg_restore --username=$databaseUser --clean --if-exists --no-owner --dbname=$databaseName $containerDumpPath
    } else {
        pg_restore --clean --if-exists --no-owner --dbname=$env:DATABASE_TEST_URL (Join-Path $restore "postgres.dump")
    }
    if ($LASTEXITCODE -ne 0) { throw "PostgreSQL restore failed." }
    $env:DATABASE_URL = $env:DATABASE_TEST_URL
    cargo run --quiet -p lxcup-persistence --bin migrate
    if ($LASTEXITCODE -ne 0) { throw "Restored database migrations failed." }

    function Invoke-RestorePsql([string]$Sql) {
        if ([string]::IsNullOrWhiteSpace($PostgresToolsContainer)) {
            $result = psql --dbname=$env:DATABASE_TEST_URL --tuples-only --no-align --command $Sql
        } else {
            $result = docker exec $PostgresToolsContainer psql --username=$databaseUser --dbname=$databaseName --tuples-only --no-align --command $Sql
        }
        if ($LASTEXITCODE -ne 0) { throw "Restored PostgreSQL data could not be validated." }
        return $result
    }

    if (-not [string]::IsNullOrWhiteSpace($PostgresToolsContainer)) {
        $connectedDatabase = Invoke-RestorePsql "SELECT current_database();"
        if ($connectedDatabase -notcontains $databaseName) { throw "PostgresToolsContainer is not connected to DATABASE_TEST_URL's database." }
    }

    $integritySql = @"
WITH required_tables(name) AS (VALUES ('targets'), ('audit_events'), ('package_inventory_snapshots')),
missing_tables AS (
    SELECT name FROM required_tables
    WHERE to_regclass('public.' || name) IS NULL
), bad_secret_refs AS (
    SELECT secret_ref FROM (
        SELECT payload->>'credential_secret_ref' AS secret_ref FROM targets
        UNION SELECT payload->>'ssh_known_hosts_secret_ref' FROM targets
        UNION SELECT payload->>'agent_secret_ref' FROM targets
        UNION SELECT secret_ref::text FROM agent_registrations
        UNION SELECT ca_secret_ref::text FROM agent_registrations WHERE ca_secret_ref IS NOT NULL
    ) refs
    WHERE secret_ref IS NOT NULL AND secret_ref !~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
)
SELECT 'missing_tables=' || (SELECT count(*) FROM missing_tables)
UNION ALL SELECT 'bad_secret_refs=' || (SELECT count(*) FROM bad_secret_refs)
UNION ALL SELECT 'audit_rows=' || (SELECT count(*) FROM audit_events)
UNION ALL SELECT 'audit_data_invalid=' || (SELECT count(*) FROM audit_events WHERE event_type IS NULL OR jsonb_typeof(details) <> 'object' OR created_at IS NULL);
"@
    $checks = Invoke-RestorePsql $integritySql
    if ($checks -notcontains 'missing_tables=0' -or $checks -notcontains 'bad_secret_refs=0' -or $checks -notcontains 'audit_data_invalid=0') { throw "Restored database schema, secret-reference shape, or audit data validation failed." }

    $secretRoot = Join-Path $restore "secrets"
    if (-not (Test-Path -LiteralPath $secretRoot -PathType Container)) { throw "Encrypted secret-store payload is missing." }
    $secretMetadata = @{}
    foreach ($metadataFile in Get-ChildItem -LiteralPath $secretRoot -Filter "*.json" -File) {
        $secretMetadata[$metadataFile.BaseName] = Get-Content -LiteralPath $metadataFile.FullName -Raw | ConvertFrom-Json
    }
    $referencedSecretIds = Invoke-RestorePsql @"
SELECT DISTINCT secret_ref FROM (
    SELECT payload->>'credential_secret_ref' AS secret_ref FROM targets
    UNION SELECT payload->>'ssh_known_hosts_secret_ref' FROM targets
    UNION SELECT payload->>'agent_secret_ref' FROM targets
    UNION SELECT secret_ref::text FROM agent_registrations
    UNION SELECT ca_secret_ref::text FROM agent_registrations WHERE ca_secret_ref IS NOT NULL
) refs WHERE secret_ref IS NOT NULL
"@
    foreach ($secretId in $referencedSecretIds) {
        if (-not $secretMetadata.ContainsKey($secretId)) { throw "A database secret reference has no restored metadata." }
        if (-not (Test-Path -LiteralPath (Join-Path $secretRoot "$secretId.enc") -PathType Leaf)) { throw "A restored secret has no encrypted value file." }
        if ($secretMetadata[$secretId].status -ne "active") { throw "A database secret reference points to a non-active secret." }
    }

    if ([string]::IsNullOrWhiteSpace($env:LXCUP_SECRET_MASTER_KEY)) { throw "LXCUP_SECRET_MASTER_KEY must be provided through the external secret manager for decryption validation." }
    $env:LXCUP_SECRET_STORE_DIR = $secretRoot
    cargo run --quiet -p lxcup-secrets --example verify_store
    if ($LASTEXITCODE -ne 0) { throw "Encrypted secret-store validation failed." }
    Write-Host "Isolated restore test succeeded: migrations applied, target/secret references checked, and audit rows readable. Secret values were not printed."
}

finally {
    if ($containerDumpPath) { docker exec $PostgresToolsContainer rm -f $containerDumpPath 2>$null | Out-Null }
    if ($null -eq $previousDatabaseUrl) { Remove-Item Env:DATABASE_URL -ErrorAction SilentlyContinue } else { $env:DATABASE_URL = $previousDatabaseUrl }
    if ($null -eq $previousSecretStoreDir) { Remove-Item Env:LXCUP_SECRET_STORE_DIR -ErrorAction SilentlyContinue } else { $env:LXCUP_SECRET_STORE_DIR = $previousSecretStoreDir }
    if (Test-Path -LiteralPath $restore) { Remove-Item -LiteralPath $restore -Recurse -Force }
}
