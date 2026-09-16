# Rust-Setup für lxcup unter Windows

Für den aktuellen Entwicklungsstand muss nur die Rust-Toolchain eingerichtet
werden. Git, GitHub CLI, Node.js und Docker sind bereits vorhanden.

## 1. Visual Studio Build Tools installieren

Rust verwendet unter Windows den MSVC-Compiler. Installiere deshalb die
[Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/).

Im Installer auswählen:

- Desktop development with C++
- MSVC Build Tools
- Windows 10 oder Windows 11 SDK

Danach PowerShell neu öffnen.

## 2. Rust über rustup installieren

Die offizielle Installationsseite ist:

<https://rustup.rs/>

Unter Windows `rustup-init.exe` herunterladen und starten.

Die Standardauswahl verwenden:

```text
1) Proceed with installation (default)
x86_64-pc-windows-msvc
```

Danach ein neues PowerShell-Fenster öffnen.

## 3. Installation prüfen

```powershell
rustc --version
cargo --version
rustup --version
```

## 4. Projekttoolchain installieren

Im Repository-Root ausführen:

```powershell
rustup toolchain install 1.85.0-x86_64-pc-windows-msvc
rustup component add rustfmt clippy --toolchain 1.85.0-x86_64-pc-windows-msvc
```

Das Projekt verwendet die in `rust-toolchain.toml` festgelegte Version
automatisch.

## 5. lxcup prüfen

Im Repository-Root:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Alle drei Befehle müssen erfolgreich durchlaufen.

## Häufige Probleme

### `cargo` oder `rustc` wird nicht gefunden

PowerShell schließen und neu öffnen. Falls der Fehler bleibt, prüfen, ob
Rustup installiert wurde und der Rust-Binärpfad in `PATH` enthalten ist.

### Linker- oder MSVC-Fehler

Visual Studio Build Tools erneut öffnen und sicherstellen, dass MSVC und das
Windows SDK installiert sind.

### Toolchain fehlt

```powershell
rustup show
rustup toolchain install 1.85.0-x86_64-pc-windows-msvc
```
