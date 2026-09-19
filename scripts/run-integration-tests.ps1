param(
    [switch]$NoBuild
)

$ErrorActionPreference = "Stop"

function Import-DotEnv {
    param([string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) {
        return
    }

    foreach ($line in Get-Content -LiteralPath $Path) {
        $trimmed = $line.Trim()
        if ([string]::IsNullOrWhiteSpace($trimmed) -or $trimmed.StartsWith("#")) {
            continue
        }

        if ($trimmed.StartsWith("export ")) {
            $trimmed = $trimmed.Substring(7).TrimStart()
        }

        if ($trimmed -notmatch "^(?<name>[A-Za-z_][A-Za-z0-9_]*)=(?<value>.*)$") {
            continue
        }

        $name = $Matches.name
        $value = $Matches.value.Trim()
        if (($value.StartsWith('"') -and $value.EndsWith('"')) -or
            ($value.StartsWith("'") -and $value.EndsWith("'"))) {
            $value = $value.Substring(1, $value.Length - 2)
        }

        $existing = (Get-Item "Env:$name" -ErrorAction SilentlyContinue).Value
        if ([string]::IsNullOrWhiteSpace($existing)) {
            [Environment]::SetEnvironmentVariable($name, $value, "Process")
        }
    }
}

Import-DotEnv -Path (Join-Path $PSScriptRoot "..\.env")

$required = @(
    "DATABASE_TEST_URL",
    "PROXMOX_TEST_BASE_URL",
    "PROXMOX_TEST_TOKEN_ID",
    "PROXMOX_TEST_TOKEN_SECRET",
    "LXCUP_INTEGRATION_NODE",
    "LXCUP_INTEGRATION_VMID"
)

foreach ($name in $required) {
    if ([string]::IsNullOrWhiteSpace((Get-Item "Env:$name" -ErrorAction SilentlyContinue).Value)) {
        throw "$name must be set for the opt-in integration harness."
    }
}

if (-not [string]::IsNullOrWhiteSpace($env:DATABASE_URL) -and $env:DATABASE_TEST_URL -eq $env:DATABASE_URL) {
    throw "DATABASE_TEST_URL must not equal DATABASE_URL."
}

if (-not $NoBuild) {
    cargo build --workspace
}

cargo test -p lxcup-test-support --test dedicated_lxc -- --ignored --nocapture
