//! Austauschbare Secret-Store-Grenze für Backend und Ansible-Ausführung.
//!
//! Dieses Crate kennt keine HTTP- oder PostgreSQL-Details. Provider liefern
//! ausschließlich Metadaten oder explizit angefordertes Secret-Material.
//! Secret-Material ist nicht serialisierbar und wird niemals debug-gedruckt.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use lxcup_core::{SecretId, SecretKind, SecretMetadata, SecretScope, SecretValue};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Status of a stored credential.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretStatus {
    Active,
    Revoked,
}

/// Metadata returned by providers. It never contains the secret value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StoredSecretMetadata {
    pub metadata: SecretMetadata,
    pub status: SecretStatus,
}

/// Input for creating a secret.
#[derive(Clone)]
pub struct CreateSecret {
    pub name: String,
    pub kind: SecretKind,
    pub scope: SecretScope,
    pub value: SecretValue,
}

/// Secret-store errors use stable categories and intentionally omit provider
/// details, paths and values from their display text.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum SecretStoreError {
    #[error("secret is missing")]
    Missing,
    #[error("secret access is denied")]
    Denied,
    #[error("secret request is invalid")]
    Invalid,
    #[error("secret provider is unavailable")]
    Unavailable,
}

/// Small provider interface used by the application and replaceable by fakes.
pub trait SecretStore: Send + Sync {
    fn create(&self, request: CreateSecret) -> Result<StoredSecretMetadata, SecretStoreError>;
    fn read(&self, id: SecretId) -> Result<SecretValue, SecretStoreError>;
    fn metadata(&self, id: SecretId) -> Result<StoredSecretMetadata, SecretStoreError>;
    fn update(&self, id: SecretId, value: SecretValue) -> Result<(), SecretStoreError>;
    fn rotate(&self, id: SecretId, value: SecretValue) -> Result<(), SecretStoreError>;
    fn revoke(&self, id: SecretId) -> Result<(), SecretStoreError>;
    fn delete(&self, id: SecretId) -> Result<(), SecretStoreError>;
}

#[derive(Clone)]
struct SecretRecord {
    metadata: StoredSecretMetadata,
    value: SecretValue,
}

/// Deterministic provider for unit and application tests.
#[derive(Clone, Default)]
pub struct InMemorySecretStore {
    records: Arc<RwLock<HashMap<SecretId, SecretRecord>>>,
}

impl SecretStore for InMemorySecretStore {
    fn create(&self, request: CreateSecret) -> Result<StoredSecretMetadata, SecretStoreError> {
        let metadata = SecretMetadata::new(request.name, request.kind, request.scope)
            .map_err(|_| SecretStoreError::Invalid)?;
        let stored = StoredSecretMetadata {
            metadata,
            status: SecretStatus::Active,
        };
        let id = stored.metadata.id;
        let result = stored.clone();
        self.records
            .write()
            .map_err(|_| SecretStoreError::Unavailable)?
            .insert(
                id,
                SecretRecord {
                    metadata: stored,
                    value: request.value,
                },
            );
        Ok(result)
    }

    fn read(&self, id: SecretId) -> Result<SecretValue, SecretStoreError> {
        let records = self
            .records
            .read()
            .map_err(|_| SecretStoreError::Unavailable)?;
        let record = records.get(&id).ok_or(SecretStoreError::Missing)?;
        if record.metadata.status != SecretStatus::Active {
            return Err(SecretStoreError::Denied);
        }
        Ok(record.value.clone())
    }

    fn metadata(&self, id: SecretId) -> Result<StoredSecretMetadata, SecretStoreError> {
        self.records
            .read()
            .map_err(|_| SecretStoreError::Unavailable)?
            .get(&id)
            .map(|record| record.metadata.clone())
            .ok_or(SecretStoreError::Missing)
    }

    fn update(&self, id: SecretId, value: SecretValue) -> Result<(), SecretStoreError> {
        let mut records = self
            .records
            .write()
            .map_err(|_| SecretStoreError::Unavailable)?;
        let record = records.get_mut(&id).ok_or(SecretStoreError::Missing)?;
        if record.metadata.status != SecretStatus::Active {
            return Err(SecretStoreError::Denied);
        }
        record.value = value;
        record.metadata.metadata.updated_at = chrono::Utc::now();
        Ok(())
    }

    fn rotate(&self, id: SecretId, value: SecretValue) -> Result<(), SecretStoreError> {
        self.update(id, value)
    }

    fn revoke(&self, id: SecretId) -> Result<(), SecretStoreError> {
        let mut records = self
            .records
            .write()
            .map_err(|_| SecretStoreError::Unavailable)?;
        let record = records.get_mut(&id).ok_or(SecretStoreError::Missing)?;
        record.metadata.status = SecretStatus::Revoked;
        record.metadata.metadata.updated_at = chrono::Utc::now();
        Ok(())
    }

    fn delete(&self, id: SecretId) -> Result<(), SecretStoreError> {
        self.records
            .write()
            .map_err(|_| SecretStoreError::Unavailable)?
            .remove(&id)
            .map(|_| ())
            .ok_or(SecretStoreError::Missing)
    }
}

/// Development provider backed by files outside the repository. Pointing it
/// at `/run/secrets` also supports Docker Secret mounts as a read-only source.
#[derive(Clone)]
pub struct FileSecretStore {
    root: PathBuf,
}

impl FileSecretStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, SecretStoreError> {
        let root = root.into();
        if root.as_os_str().is_empty() {
            return Err(SecretStoreError::Invalid);
        }
        fs::create_dir_all(&root).map_err(|_| SecretStoreError::Unavailable)?;
        Ok(Self { root })
    }

    fn value_path(&self, id: SecretId) -> PathBuf {
        self.root.join(format!("{}.secret", id.as_uuid()))
    }

    fn metadata_path(&self, id: SecretId) -> PathBuf {
        self.root.join(format!("{}.json", id.as_uuid()))
    }

    fn write_value(&self, id: SecretId, value: &SecretValue) -> Result<(), SecretStoreError> {
        write_secret_file(&self.value_path(id), value.expose())
    }

    fn read_metadata(&self, id: SecretId) -> Result<StoredSecretMetadata, SecretStoreError> {
        let bytes = fs::read(self.metadata_path(id)).map_err(map_io_error)?;
        serde_json::from_slice(&bytes).map_err(|_| SecretStoreError::Invalid)
    }

    fn write_metadata(&self, metadata: &StoredSecretMetadata) -> Result<(), SecretStoreError> {
        let bytes = serde_json::to_vec(metadata).map_err(|_| SecretStoreError::Invalid)?;
        fs::write(self.metadata_path(metadata.metadata.id), bytes).map_err(map_io_error)
    }
}

impl SecretStore for FileSecretStore {
    fn create(&self, request: CreateSecret) -> Result<StoredSecretMetadata, SecretStoreError> {
        let metadata = SecretMetadata::new(request.name, request.kind, request.scope)
            .map_err(|_| SecretStoreError::Invalid)?;
        let stored = StoredSecretMetadata {
            metadata,
            status: SecretStatus::Active,
        };
        self.write_metadata(&stored)?;
        if let Err(error) = self.write_value(stored.metadata.id, &request.value) {
            let _ = fs::remove_file(self.metadata_path(stored.metadata.id));
            return Err(error);
        }
        Ok(stored)
    }

    fn read(&self, id: SecretId) -> Result<SecretValue, SecretStoreError> {
        let metadata = self.read_metadata(id)?;
        if metadata.status != SecretStatus::Active {
            return Err(SecretStoreError::Denied);
        }
        let value = fs::read_to_string(self.value_path(id)).map_err(map_io_error)?;
        SecretValue::new(value).map_err(|_| SecretStoreError::Invalid)
    }

    fn metadata(&self, id: SecretId) -> Result<StoredSecretMetadata, SecretStoreError> {
        self.read_metadata(id)
    }

    fn update(&self, id: SecretId, value: SecretValue) -> Result<(), SecretStoreError> {
        let mut metadata = self.read_metadata(id)?;
        if metadata.status != SecretStatus::Active {
            return Err(SecretStoreError::Denied);
        }
        self.write_value(id, &value)?;
        metadata.metadata.updated_at = chrono::Utc::now();
        self.write_metadata(&metadata)
    }

    fn rotate(&self, id: SecretId, value: SecretValue) -> Result<(), SecretStoreError> {
        self.update(id, value)
    }

    fn revoke(&self, id: SecretId) -> Result<(), SecretStoreError> {
        let mut metadata = self.read_metadata(id)?;
        metadata.status = SecretStatus::Revoked;
        metadata.metadata.updated_at = chrono::Utc::now();
        self.write_metadata(&metadata)
    }

    fn delete(&self, id: SecretId) -> Result<(), SecretStoreError> {
        self.read_metadata(id)?;
        fs::remove_file(self.value_path(id)).map_err(map_io_error)?;
        fs::remove_file(self.metadata_path(id)).map_err(map_io_error)
    }
}

/// Read-only provider for Docker Secret mounts such as `/run/secrets`.
/// Docker owns rotation and deletion; lxcup only resolves declared entries.
#[derive(Clone)]
pub struct DockerSecretStore {
    entries: Arc<HashMap<SecretId, (StoredSecretMetadata, PathBuf)>>,
}

/// Declaration connecting a safe lxcup secret reference to a mounted file.
#[derive(Clone, Debug)]
pub struct DockerSecretDefinition {
    pub name: String,
    pub file_name: String,
    pub kind: SecretKind,
    pub scope: SecretScope,
}

impl DockerSecretStore {
    pub fn new(
        root: impl Into<PathBuf>,
        definitions: impl IntoIterator<Item = DockerSecretDefinition>,
    ) -> Result<Self, SecretStoreError> {
        let root = root.into();
        if root.as_os_str().is_empty() {
            return Err(SecretStoreError::Invalid);
        }
        let mut entries = HashMap::new();
        for definition in definitions {
            if definition.file_name.is_empty()
                || definition.file_name.contains(['/', '\\', ':'])
                || definition.file_name == "."
                || definition.file_name == ".."
            {
                return Err(SecretStoreError::Invalid);
            }
            let metadata = SecretMetadata::new(definition.name, definition.kind, definition.scope)
                .map_err(|_| SecretStoreError::Invalid)?;
            entries.insert(
                metadata.id,
                (
                    StoredSecretMetadata {
                        metadata,
                        status: SecretStatus::Active,
                    },
                    root.join(definition.file_name),
                ),
            );
        }
        Ok(Self {
            entries: Arc::new(entries),
        })
    }

    fn entry(&self, id: SecretId) -> Result<&(StoredSecretMetadata, PathBuf), SecretStoreError> {
        self.entries.get(&id).ok_or(SecretStoreError::Missing)
    }
}

impl SecretStore for DockerSecretStore {
    fn create(&self, _request: CreateSecret) -> Result<StoredSecretMetadata, SecretStoreError> {
        Err(SecretStoreError::Denied)
    }

    fn read(&self, id: SecretId) -> Result<SecretValue, SecretStoreError> {
        let (metadata, path) = self.entry(id)?;
        if metadata.status != SecretStatus::Active {
            return Err(SecretStoreError::Denied);
        }
        let value = fs::read_to_string(path).map_err(map_io_error)?;
        SecretValue::new(value).map_err(|_| SecretStoreError::Invalid)
    }

    fn metadata(&self, id: SecretId) -> Result<StoredSecretMetadata, SecretStoreError> {
        Ok(self.entry(id)?.0.clone())
    }

    fn update(&self, _id: SecretId, _value: SecretValue) -> Result<(), SecretStoreError> {
        Err(SecretStoreError::Denied)
    }

    fn rotate(&self, _id: SecretId, _value: SecretValue) -> Result<(), SecretStoreError> {
        Err(SecretStoreError::Denied)
    }

    fn revoke(&self, _id: SecretId) -> Result<(), SecretStoreError> {
        Err(SecretStoreError::Denied)
    }

    fn delete(&self, _id: SecretId) -> Result<(), SecretStoreError> {
        Err(SecretStoreError::Denied)
    }
}

fn write_secret_file(path: &Path, value: &str) -> Result<(), SecretStoreError> {
    fs::write(path, value).map_err(map_io_error)?;
    set_private_permissions(path)?;
    Ok(())
}

fn map_io_error(error: std::io::Error) -> SecretStoreError {
    match error.kind() {
        std::io::ErrorKind::NotFound => SecretStoreError::Missing,
        std::io::ErrorKind::PermissionDenied => SecretStoreError::Denied,
        _ => SecretStoreError::Unavailable,
    }
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<(), SecretStoreError> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path).map_err(map_io_error)?.permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions).map_err(map_io_error)
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> Result<(), SecretStoreError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        CreateSecret, DockerSecretDefinition, DockerSecretStore, FileSecretStore,
        InMemorySecretStore, SecretStatus, SecretStore,
    };
    use lxcup_core::{SecretKind, SecretScope, SecretValue};

    fn request(value: &str) -> CreateSecret {
        CreateSecret {
            name: "proxmox-test".to_owned(),
            kind: SecretKind::ProxmoxApiToken,
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
    fn errors_are_stable_and_redacted() {
        let error = super::SecretStoreError::Unavailable;
        assert_eq!(error.to_string(), "secret provider is unavailable");
        assert!(!error.to_string().contains("password"));
    }

    #[test]
    fn docker_secret_provider_is_read_only_and_path_safe() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("proxmox-token"), "docker-secret").unwrap();
        let store = DockerSecretStore::new(
            directory.path(),
            [DockerSecretDefinition {
                name: "proxmox-test".to_owned(),
                file_name: "proxmox-token".to_owned(),
                kind: SecretKind::ProxmoxApiToken,
                scope: SecretScope::Global,
            }],
        )
        .unwrap();
        let id = store
            .metadata(store.entries.keys().next().copied().unwrap())
            .unwrap()
            .metadata
            .id;

        assert_eq!(store.read(id).unwrap().expose(), "docker-secret");
        assert_eq!(
            store.update(id, SecretValue::new("new").unwrap()),
            Err(super::SecretStoreError::Denied)
        );
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
}
