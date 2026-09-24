[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$AgeRecipient,
    [string]$BackupDirectory = ".\backups",
    [int]$RetentionDays = 30
)

$ErrorActionPreference = "Stop"
if ([string]::IsNullOrWhiteSpace($AgeRecipient)) { throw "AgeRecipient must be provided; the private key is never read by this script." }
if (-not (Get-Command age -ErrorAction SilentlyContinue)) { throw "age is required for encrypted backups." }

$root = (Resolve-Path ".").Path
$staging = Join-Path ([IO.Path]::GetTempPath()) ("lxcup-backup-" + [guid]::NewGuid())
$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$output = Join-Path (Resolve-Path (New-Item -ItemType Directory -Force -Path $BackupDirectory)) "lxcup-$timestamp.tar.age"
New-Item -ItemType Directory -Force -Path $staging | Out-Null
try {
    # pg_dump receives credentials through the Compose container environment;
    # no URL or password is written to PowerShell output.
    docker compose exec -T postgres sh -lc 'pg_dump --format=custom "$POSTGRES_DB"' > (Join-Path $staging "postgres.dump")
    if ($LASTEXITCODE -ne 0) { throw "PostgreSQL backup failed." }
    docker compose cp "lxcup-server:/var/lib/lxcup/secrets" (Join-Path $staging "secrets")
    if ($LASTEXITCODE -ne 0) { throw "Encrypted secret-store export failed." }
    tar -cf - -C $staging postgres.dump secrets | age --encrypt --recipient $AgeRecipient --output $output
    if ($LASTEXITCODE -ne 0) { throw "Encrypted backup creation failed." }
    Get-ChildItem -LiteralPath (Resolve-Path $BackupDirectory) -Filter "lxcup-*.tar.age" |
        Where-Object LastWriteTime -lt (Get-Date).AddDays(-$RetentionDays) |
        Remove-Item -Force
    Write-Host "Encrypted backup created: $output"
}
finally {
    if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force }
}
