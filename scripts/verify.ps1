$ErrorActionPreference = "Stop"

Write-Host "Checking formatting..."
cargo fmt --all -- --check

Write-Host "Running Clippy..."
cargo clippy --workspace --all-targets --all-features -- -D warnings

Write-Host "Building workspace..."
cargo build --workspace

Write-Host "Running tests..."
cargo test --workspace

Write-Host "All Rust checks passed."
