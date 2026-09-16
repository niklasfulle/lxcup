$ErrorActionPreference = "Stop"

Write-Host "Checking formatting..."
cargo fmt --all -- --check

Write-Host "Running Clippy..."
cargo clippy --workspace --all-targets --all-features -- -D warnings

Write-Host "Building workspace..."
cargo build --workspace

Write-Host "Running tests..."
cargo test --workspace

if (Test-Path "frontend/package.json") {
    Write-Host "Running frontend tests and production build..."
    Push-Location frontend
    if (-not (Test-Path "node_modules")) {
        npm ci
    }
    npm test
    npm run build
    Pop-Location
}

Write-Host "All Rust and frontend checks passed."
