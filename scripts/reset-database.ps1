[CmdletBinding()]
param(
    [ValidateSet("postgres", "postgres-test")]
    [string]$Service = "postgres",
    [switch]$Force
)

$ErrorActionPreference = "Stop"

if (-not $Force) {
    Write-Host "ACHTUNG: Das leert die komplette Datenbank im Compose-Service '$Service' und den lxcup-Secret-Store."
    Write-Host "Alle Daten, Secret-Werte und die SQLx-Migrationshistorie werden gelöscht."
    $confirmation = Read-Host "Zum Fortfahren exakt 'DELETE $Service AND SECRETS' eingeben"
    if ($confirmation -cne "DELETE $Service AND SECRETS") {
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

Write-Host "Leere Secret-Store ..."
docker compose run --rm --no-deps --entrypoint /bin/sh lxcup-server -lc 'find /var/lib/lxcup/secrets -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +'
if ($LASTEXITCODE -ne 0) {
    throw "Secret-Store konnte nicht geleert werden (Exit-Code $LASTEXITCODE)."
}

Write-Host "Datenbank und Secret-Store geleert. Die Anwendung legt Migrationen beim nächsten Start wieder an."
Write-Host "Starte Server und Worker in der richtigen Reihenfolge neu ..."
docker compose restart lxcup-server
if ($LASTEXITCODE -ne 0) {
    throw "lxcup-server konnte nicht neu gestartet werden (Exit-Code $LASTEXITCODE)."
}
docker compose up -d --force-recreate lxcup-worker
if ($LASTEXITCODE -ne 0) {
    throw "lxcup-worker konnte nicht neu erstellt werden (Exit-Code $LASTEXITCODE)."
}

Write-Host "Reset abgeschlossen. Servermigrationen und Worker sind wieder aktiv."
