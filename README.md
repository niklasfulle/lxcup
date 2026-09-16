# lxcup

Webbasierter Update-Manager für Proxmox-LXC-Container.

## Workspace

```text
crates/
├── lxcup-core    Domain- und Geschäftslogik
├── lxcup-cli     optionale CLI-Oberfläche
└── lxcup-server  Backend/API und spätere Webauslieferung
```

Die Infrastrukturadapter für Proxmox, APT, Docker und Windows werden ergänzt,
sobald die zugehörigen MVP-Tickets umgesetzt werden. Sie dürfen keine
Geschäftslogik aus `lxcup-core` duplizieren.

## Lokale Prüfungen

Die Rust-Version ist in `rust-toolchain.toml` festgelegt.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

## Entwicklungsprinzip

Verändernde Aktionen folgen immer:

```text
SCAN → PLAN → BESTÄTIGUNG → APPLY
```
