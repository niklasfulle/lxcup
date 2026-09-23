[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [string]$Address,

    [int]$Port = 22,

    [switch]$UseWorker
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($Address)) {
    $Address = Read-Host "SSH-Adresse oder Hostname"
}
if ([string]::IsNullOrWhiteSpace($Address)) {
    throw "Eine SSH-Adresse ist erforderlich."
}
if ($Port -lt 1 -or $Port -gt 65535) {
    throw "Der Port muss zwischen 1 und 65535 liegen."
}

if (-not $UseWorker) {
    $scanner = Get-Command ssh-keyscan -ErrorAction SilentlyContinue
}
if (-not $UseWorker -and -not $scanner) {
    throw "ssh-keyscan wurde nicht gefunden. Installiere den Windows OpenSSH Client oder führe das Skript in der Worker-Umgebung aus."
}

Write-Host "Lese SSH-Hostschlüssel von $Address`:$Port ..." -ForegroundColor Cyan
$lines = if ($UseWorker) {
    $compose = Get-Command docker -ErrorAction SilentlyContinue
    if (-not $compose) { throw "Docker wurde nicht gefunden; entferne -UseWorker oder starte Docker Desktop." }
    @(& $compose.Source compose exec -T lxcup-worker ssh-keyscan -T 5 -p $Port $Address 2>$null)
} else {
    @(& $scanner.Source -T 5 -p $Port $Address 2>$null)
}
if ($LASTEXITCODE -ne 0 -or $lines.Count -eq 0) {
    throw "Kein SSH-Hostschlüssel erreichbar. Prüfe Adresse, Port und Firewall."
}

$knownHosts = ($lines -join [Environment]::NewLine).Trim()
$fingerprints = @($lines | ForEach-Object {
    $_ | ssh-keygen -lf - -E sha256 2>$null
})

Write-Host "`nEintrag für das Secret (vollständig kopieren):" -ForegroundColor Green
Write-Output $knownHosts
if ($fingerprints.Count -gt 0) {
    Write-Host "`nFingerprints:" -ForegroundColor Green
    $fingerprints | Write-Output
}

try {
    Set-Clipboard -Value $knownHosts
    Write-Host "`nDer Known-Hosts-Eintrag wurde in die Zwischenablage kopiert." -ForegroundColor Yellow
} catch {
    Write-Host "`nZwischenablage nicht verfügbar; kopiere den Eintrag manuell." -ForegroundColor Yellow
}

$save = Read-Host "Zusätzlich als Datei speichern? (Pfad leer lassen zum Überspringen)"
if (-not [string]::IsNullOrWhiteSpace($save)) {
    Set-Content -LiteralPath $save -Value $knownHosts -Encoding utf8NoBOM
    Write-Host "Gespeichert: $save" -ForegroundColor Green
}
