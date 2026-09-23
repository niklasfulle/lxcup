[CmdletBinding()]
param(
    [ValidateSet("postgres", "postgres-test")]
    [string]$Service = "postgres",
    [switch]$Force
)

$ErrorActionPreference = "Stop"

if (-not $Force) {
    Write-Host "ACHTUNG: Das leert die komplette Datenbank im Compose-Service '$Service'."
    Write-Host "Alle Daten, inklusive SQLx-Migrationshistorie, werden gelöscht."
    $confirmation = Read-Host "Zum Fortfahren exakt 'DELETE $Service' eingeben"
    if ($confirmation -cne "DELETE $Service") {
        Write-Host "Abgebrochen."
        exit 1
    }
}

$sql = @'
DROP SCHEMA IF EXISTS public CASCADE;
CREATE SCHEMA public;
GRANT ALL ON SCHEMA public TO PUBLIC;
'@

Write-Host "Leere Datenbank im Service '$Service' ..."
$sql | docker compose exec -T $Service sh -lc 'psql -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB"'
if ($LASTEXITCODE -ne 0) {
    throw "Datenbank konnte nicht geleert werden (Exit-Code $LASTEXITCODE). Läuft der Compose-Stack?"
}

Write-Host "Datenbank geleert. Die Anwendung legt ihre Migrationen beim nächsten Start wieder an."
