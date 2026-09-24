[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$AgeRecipient,
    [string]$BackupDirectory = ".\backups",
    [ValidateRange(1, 3650)]
    [int]$RetentionDays = 30
)

$ErrorActionPreference = "Stop"
Write-Warning "This compatibility command creates a full encrypted stack backup; standalone plaintext database dumps are no longer created."
& (Join-Path $PSScriptRoot "backup-stack.ps1") -AgeRecipient $AgeRecipient -BackupDirectory $BackupDirectory -RetentionDays $RetentionDays
