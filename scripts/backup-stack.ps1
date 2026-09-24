[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$AgeRecipient,
    [string]$BackupDirectory = ".\backups",
    [ValidateRange(1, 3650)]
    [int]$RetentionDays = 30
)

$ErrorActionPreference = "Stop"
function Protect-PrivateDirectory([string]$Path) {
    if ($env:OS -eq "Windows_NT") {
        $identity = [Security.Principal.WindowsIdentity]::GetCurrent().Name
        & icacls $Path /inheritance:r /grant:r "${identity}:(OI)(CI)F" /Q | Out-Null
    } else {
        & chmod 700 -- $Path
    }
    if ($LASTEXITCODE -ne 0) { throw "Could not restrict access to temporary backup data." }
}

if ([string]::IsNullOrWhiteSpace($AgeRecipient)) { throw "AgeRecipient must be provided; the private key is never read by this script." }
if (-not (Get-Command age -ErrorAction SilentlyContinue)) { throw "age is required for encrypted backups." }

$staging = Join-Path ([IO.Path]::GetTempPath()) ("lxcup-backup-" + [guid]::NewGuid())
$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$output = Join-Path (Resolve-Path (New-Item -ItemType Directory -Force -Path $BackupDirectory)) "lxcup-$timestamp.tar.age"
try {
    New-Item -ItemType Directory -Force -Path $staging | Out-Null
    Protect-PrivateDirectory $staging
    $archive = Join-Path $staging "backup.tar"
    # pg_dump receives credentials through the Compose container environment;
    # no URL or password is written to PowerShell output.
    docker compose exec -T postgres sh -lc 'pg_dump --username="$POSTGRES_USER" --format=custom "$POSTGRES_DB"' > (Join-Path $staging "postgres.dump")
    if ($LASTEXITCODE -ne 0) { throw "PostgreSQL backup failed." }
    docker compose cp "lxcup-server:/var/lib/lxcup/secrets" (Join-Path $staging "secrets")
    if ($LASTEXITCODE -ne 0) { throw "Encrypted secret-store export failed." }
    tar -cf $archive -C $staging postgres.dump secrets
    if ($LASTEXITCODE -ne 0) { throw "Backup archive creation failed." }
    age --encrypt --recipient $AgeRecipient --output $output $archive
    if ($LASTEXITCODE -ne 0) { throw "Encrypted backup creation failed." }
    if (-not (Test-Path -LiteralPath $output) -or (Get-Item -LiteralPath $output).Length -eq 0) { throw "Encrypted backup output is empty." }
    Get-ChildItem -LiteralPath (Resolve-Path $BackupDirectory) -Filter "lxcup-*.tar.age" |
        Where-Object LastWriteTime -lt (Get-Date).AddDays(-$RetentionDays) |
        Remove-Item -Force
    Write-Host "Encrypted backup created: $output"
}

finally {
    if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force }
}
