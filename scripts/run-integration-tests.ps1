param(
    [switch]$NoBuild
)

$ErrorActionPreference = "Stop"

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
