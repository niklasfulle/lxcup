use super::{
    CreateSecret, DockerSecretDefinition, DockerSecretStore, EncryptedFileSecretStore,
    FileSecretStore, InMemorySecretStore, SecretMasterKey, SecretStatus, SecretStore,
};
use lxcup_core::{SecretId, SecretKind, SecretScope, SecretValue};

#[test]
fn master_key_hex_parser_validates_length_digits_and_redacts_debug() {
    let key = SecretMasterKey::from_hex(&format!(" {} ", "ab".repeat(32))).expect("valid hex key");
    assert_eq!(format!("{key:?}"), "SecretMasterKey([REDACTED])");
    assert_eq!(
        SecretMasterKey::from_hex("ab"),
        Err(super::SecretStoreError::Invalid)
    );
    assert_eq!(
        SecretMasterKey::from_hex(&format!("{}zz", "ab".repeat(31))),
        Err(super::SecretStoreError::Invalid)
    );
}

fn request(value: &str) -> CreateSecret {
    CreateSecret {
        name: "connection-test".to_owned(),
        kind: SecretKind::Generic,
        scope: SecretScope::Global,
        value: SecretValue::new(value).unwrap(),
    }
}

#[test]
fn in_memory_provider_supports_lifecycle_without_leaking_values() {
    let store = InMemorySecretStore::default();
    let created = store.create(request("top-secret")).unwrap();
    let id = created.metadata.id;

    assert_eq!(store.read(id).unwrap().expose(), "top-secret");
    assert!(!format!("{created:?}").contains("top-secret"));
    store
        .rotate(id, SecretValue::new("rotated").unwrap())
        .unwrap();
    assert_eq!(store.read(id).unwrap().expose(), "rotated");
    store.revoke(id).unwrap();
    assert_eq!(store.metadata(id).unwrap().status, SecretStatus::Revoked);
    assert_eq!(store.read(id), Err(super::SecretStoreError::Denied));
    store.delete(id).unwrap();
    assert_eq!(store.metadata(id), Err(super::SecretStoreError::Missing));
}

#[test]
fn file_provider_keeps_values_outside_repository() {
    let directory = tempfile::tempdir().unwrap();
    let store = FileSecretStore::new(directory.path()).unwrap();
    let created = store.create(request("file-secret")).unwrap();
    let path = directory
        .path()
        .join(format!("{}.secret", created.metadata.id.as_uuid()));

    assert_eq!(
        store.read(created.metadata.id).unwrap().expose(),
        "file-secret"
    );
    assert!(path.exists());
    let reopened = FileSecretStore::new(directory.path()).unwrap();
    assert_eq!(
        reopened.read(created.metadata.id).unwrap().expose(),
        "file-secret"
    );
    store.delete(created.metadata.id).unwrap();
    assert!(!path.exists());
}

#[test]
fn file_provider_covers_metadata_update_revoke_and_missing_paths() {
    let directory = tempfile::tempdir().unwrap();
    assert!(matches!(
        FileSecretStore::new(""),
        Err(super::SecretStoreError::Invalid)
    ));
    let store = FileSecretStore::new(directory.path()).unwrap();
    assert!(store.list_metadata().unwrap().is_empty());

    let created = store.create(request("file-lifecycle")).unwrap();
    let id = created.metadata.id;
    assert_eq!(store.list_metadata().unwrap(), vec![created.clone()]);
    store
        .update(id, SecretValue::new("updated-file-secret").unwrap())
        .unwrap();
    assert_eq!(store.read(id).unwrap().expose(), "updated-file-secret");
    store.revoke(id).unwrap();
    assert_eq!(store.read(id), Err(super::SecretStoreError::Denied));
    assert_eq!(
        store.update(id, SecretValue::new("blocked").unwrap()),
        Err(super::SecretStoreError::Denied)
    );
    store.delete(id).unwrap();
    assert_eq!(store.metadata(id), Err(super::SecretStoreError::Missing));
    assert_eq!(store.read(id), Err(super::SecretStoreError::Missing));
    assert_eq!(store.delete(id), Err(super::SecretStoreError::Missing));

    std::fs::write(directory.path().join("broken.json"), "not metadata").unwrap();
    assert_eq!(store.list_metadata(), Err(super::SecretStoreError::Invalid));
    assert!(matches!(
        store.create(CreateSecret {
            name: String::new(),
            ..request("ignored")
        }),
        Err(super::SecretStoreError::Invalid)
    ));
}

#[test]
fn errors_are_stable_and_redacted() {
    let error = super::SecretStoreError::Unavailable;
    assert_eq!(error.to_string(), "secret provider is unavailable");
    assert!(!error.to_string().contains("password"));
}

#[test]
fn encrypted_provider_supports_key_rotation_without_plaintext_at_rest() {
    let directory = tempfile::tempdir().unwrap();
    let old_key = SecretMasterKey::from_bytes([7; 32]);
    let new_key = SecretMasterKey::from_bytes([8; 32]);
    let store = EncryptedFileSecretStore::new(directory.path(), old_key.clone(), []).unwrap();
    let created = store.create(request("encrypted-secret")).unwrap();
    let payload = std::fs::read(
        directory
            .path()
            .join(format!("{}.enc", created.metadata.id.as_uuid())),
    )
    .unwrap();
    assert!(
        !payload
            .windows("encrypted-secret".len())
            .any(|window| { window == "encrypted-secret".as_bytes() })
    );

    let rotated =
        EncryptedFileSecretStore::new(directory.path(), new_key.clone(), [old_key]).unwrap();
    assert_eq!(
        rotated.read(created.metadata.id).unwrap().expose(),
        "encrypted-secret"
    );
    rotated.rewrap(created.metadata.id).unwrap();

    let current_only = EncryptedFileSecretStore::new(directory.path(), new_key, []).unwrap();
    assert_eq!(
        current_only.read(created.metadata.id).unwrap().expose(),
        "encrypted-secret"
    );
    assert!(
        EncryptedFileSecretStore::new(directory.path(), SecretMasterKey::from_bytes([9; 32]), [])
            .unwrap()
            .read(created.metadata.id)
            .is_err()
    );
}

#[test]
fn encrypted_provider_lists_updates_revokes_and_deletes_secret_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let store =
        EncryptedFileSecretStore::new(directory.path(), SecretMasterKey::from_bytes([4; 32]), [])
            .unwrap();
    let created = store.create(request("encrypted-lifecycle")).unwrap();
    let id = created.metadata.id;

    assert_eq!(store.list_metadata().unwrap(), vec![created]);
    assert_eq!(store.metadata(id).unwrap().metadata.name, "connection-test");
    store
        .update(id, SecretValue::new("updated-encrypted-value").unwrap())
        .unwrap();
    assert_eq!(store.read(id).unwrap().expose(), "updated-encrypted-value");
    store
        .rotate(id, SecretValue::new("rotated-encrypted-value").unwrap())
        .unwrap();
    assert_eq!(store.read(id).unwrap().expose(), "rotated-encrypted-value");
    store.revoke(id).unwrap();
    assert_eq!(store.metadata(id).unwrap().status, SecretStatus::Revoked);
    assert_eq!(store.read(id), Err(super::SecretStoreError::Denied));
    store.delete(id).unwrap();
    assert_eq!(store.metadata(id), Err(super::SecretStoreError::Missing));
}

#[test]
fn docker_secret_provider_is_read_only_and_path_safe() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("connection-token"), "docker-secret").unwrap();
    let store = DockerSecretStore::new(
        directory.path(),
        [DockerSecretDefinition {
            name: "connection-test".to_owned(),
            file_name: "connection-token".to_owned(),
            kind: SecretKind::Generic,
            scope: SecretScope::Global,
        }],
    )
    .unwrap();
    let id = store
        .metadata(store.entries.keys().next().copied().unwrap())
        .unwrap()
        .metadata
        .id;

    assert_eq!(store.list_metadata().unwrap().len(), 1);
    assert_eq!(store.read(id).unwrap().expose(), "docker-secret");
    assert!(matches!(
        store.read(SecretId::new()),
        Err(super::SecretStoreError::Missing)
    ));
    assert!(matches!(
        store.metadata(SecretId::new()),
        Err(super::SecretStoreError::Missing)
    ));
    assert_eq!(
        store.create(request("not-writable")),
        Err(super::SecretStoreError::Denied)
    );
    assert_eq!(
        store.update(id, SecretValue::new("new").unwrap()),
        Err(super::SecretStoreError::Denied)
    );
    assert_eq!(
        store.rotate(id, SecretValue::new("new").unwrap()),
        Err(super::SecretStoreError::Denied)
    );
    assert_eq!(store.revoke(id), Err(super::SecretStoreError::Denied));
    assert_eq!(store.delete(id), Err(super::SecretStoreError::Denied));
    assert!(
        DockerSecretStore::new(
            directory.path(),
            [DockerSecretDefinition {
                name: "unsafe".to_owned(),
                file_name: "../outside".to_owned(),
                kind: SecretKind::Generic,
                scope: SecretScope::Global,
            }]
        )
        .is_err()
    );
}
