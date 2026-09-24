use lxcup_secrets::{EncryptedFileSecretStore, SecretMasterKey, SecretStatus, SecretStore};

fn main() {
    if run().is_err() {
        eprintln!(
            "Encrypted secret-store validation failed; secret values and key material were omitted."
        );
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::var("LXCUP_SECRET_STORE_DIR")?;
    let key = SecretMasterKey::from_env("LXCUP_SECRET_MASTER_KEY")?;
    let store = EncryptedFileSecretStore::new(root, key, [])?;
    let metadata = store.list_metadata()?;
    let mut readable = 0_usize;
    for item in metadata {
        if item.status == SecretStatus::Active {
            // Read to verify authentication/decryption, but never format or print the value.
            let _value = store.read(item.metadata.id)?;
            readable += 1;
        }
    }
    println!("Verified {readable} active encrypted secret entries.");
    Ok(())
}
