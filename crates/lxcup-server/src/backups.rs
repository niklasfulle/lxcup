use super::{ApiEnvelope, ApiError, ApiState, envelope};
use age::{Encryptor, secrecy::SecretString};
use axum::{
    Extension, Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderValue, Response, header},
};
use chrono::{DateTime, Utc};
use lxcup_core::{ActorRole, SecretId, SecretKind, SecretScope};
use lxcup_secrets::{SecretStatus, SecretStoreError};
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    env, fs, io,
    path::{Path as FsPath, PathBuf},
    process::{Command, Stdio},
    time::SystemTime,
};
use tokio::fs::File;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

const DEFAULT_BACKUP_DIRECTORY: &str = "/var/lib/lxcup/backups";
const DEFAULT_SECRET_DIRECTORY: &str = "/var/lib/lxcup/secrets";
const DEFAULT_RETENTION_DAYS: u32 = 30;
const MAX_RETENTION_DAYS: u32 = 3650;
const MAX_LISTED_BACKUPS: usize = 100;
const MIN_BACKUP_PASSPHRASE_CHARACTERS: usize = 12;
const MAX_BACKUP_PASSPHRASE_BYTES: usize = 1024;

#[derive(Deserialize)]
pub(crate) struct CreateBackupRequest {
    confirmed: bool,
    passphrase: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub(crate) struct BackupInfo {
    pub(crate) id: Uuid,
    created_at: DateTime<Utc>,
    size_bytes: u64,
}

struct BackupPaths {
    directory: PathBuf,
    secrets: PathBuf,
    retention_days: u32,
    database_url: url::Url,
}

trait BackupToolchain {
    fn dump_database(&self, database_url: &url::Url, output: &FsPath) -> Result<(), ApiError>;
    fn create_archive(&self, workspace: &FsPath, output: &FsPath) -> Result<(), ApiError>;
    fn encrypt(
        &self,
        archive: &FsPath,
        output: &FsPath,
        passphrase: SecretString,
    ) -> Result<(), ApiError>;
}

struct SystemBackupToolchain;

impl BackupToolchain for SystemBackupToolchain {
    fn dump_database(&self, database_url: &url::Url, output: &FsPath) -> Result<(), ApiError> {
        dump_database(database_url, output)
    }

    fn create_archive(&self, workspace: &FsPath, output: &FsPath) -> Result<(), ApiError> {
        run_command(
            backup_command("tar")
                .arg("-cf")
                .arg(output)
                .arg("-C")
                .arg(workspace)
                .arg("postgres.dump")
                .arg("secrets")
                .arg("config"),
        )
        .map_err(|_| {
            ApiError::dependency(
                "backup_archive_failed",
                "backup data could not be archived; check server storage and tar availability",
            )
        })
    }

    fn encrypt(
        &self,
        archive: &FsPath,
        output: &FsPath,
        passphrase: SecretString,
    ) -> Result<(), ApiError> {
        encrypt_archive(archive, output, passphrase).map_err(|_| {
            ApiError::dependency(
                "backup_encryption_failed",
                "backup encryption failed; verify server storage availability",
            )
        })
    }
}

struct PrivateDirectory(PathBuf);

impl PrivateDirectory {
    fn create(path: PathBuf) -> Result<Self, ApiError> {
        fs::create_dir(&path).map_err(|_| backup_unavailable())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .map_err(|_| backup_unavailable())?;
        }
        Ok(Self(path))
    }
}

impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(crate) async fn list_backups(
    Extension(role): Extension<ActorRole>,
) -> Result<Json<ApiEnvelope<Vec<BackupInfo>>>, ApiError> {
    require_admin(role)?;
    list_backups_from(backup_directory()?).await
}

async fn list_backups_from(
    directory: PathBuf,
) -> Result<Json<ApiEnvelope<Vec<BackupInfo>>>, ApiError> {
    let backups = tokio::task::spawn_blocking(move || list_backup_files(&directory))
        .await
        .map_err(|_| backup_unavailable())??;
    Ok(Json(envelope(backups)))
}

pub(crate) async fn create_backup(
    State(state): State<ApiState>,
    Extension(role): Extension<ActorRole>,
    Json(mut request): Json<CreateBackupRequest>,
) -> Result<Json<ApiEnvelope<BackupInfo>>, ApiError> {
    require_admin(role)?;
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "backup_confirmation_required",
            "confirm that the encrypted backup contains sensitive system data",
        ));
    }
    validate_backup_passphrase(&request.passphrase)?;
    let passphrase = SecretString::from(std::mem::take(&mut request.passphrase));
    let backup = create_encrypted_backup_for_state(&state, passphrase).await?;
    state.publish(super::ApiEvent::status(
        "backup",
        backup.id.to_string(),
        "created",
    ));
    Ok(Json(envelope(backup)))
}

pub(crate) async fn create_scheduled_backup(
    state: &ApiState,
    secret_ref: SecretId,
) -> Result<BackupInfo, ApiError> {
    let passphrase = read_backup_passphrase(state, secret_ref)?;
    let backup = create_encrypted_backup_for_state(state, passphrase).await?;
    state.publish(super::ApiEvent::status(
        "backup",
        backup.id.to_string(),
        "created",
    ));
    Ok(backup)
}

pub(crate) fn read_backup_passphrase(
    state: &ApiState,
    secret_ref: SecretId,
) -> Result<SecretString, ApiError> {
    let metadata = state
        .secrets
        .metadata(secret_ref)
        .map_err(map_backup_secret_error)?;
    if metadata.status != SecretStatus::Active
        || metadata.metadata.kind != SecretKind::BackupPassphrase
        || metadata.metadata.scope != SecretScope::Global
    {
        return Err(ApiError::bad_request(
            "invalid_backup_secret",
            "select an active global backup-passphrase secret",
        ));
    }
    let value = state
        .secrets
        .read(secret_ref)
        .map_err(map_backup_secret_error)?;
    validate_backup_passphrase(value.expose())?;
    Ok(SecretString::from(value.expose().to_owned()))
}

fn map_backup_secret_error(error: SecretStoreError) -> ApiError {
    match error {
        SecretStoreError::Missing | SecretStoreError::Invalid | SecretStoreError::Denied => {
            ApiError::bad_request(
                "invalid_backup_secret",
                "the configured backup passphrase secret is unavailable or invalid",
            )
        }
        SecretStoreError::Unavailable => ApiError::dependency(
            "backup_secret_unavailable",
            "the backup passphrase secret store is unavailable",
        ),
    }
}

async fn create_encrypted_backup_for_state(
    state: &ApiState,
    passphrase: SecretString,
) -> Result<BackupInfo, ApiError> {
    let _guard = state.backup_gate.write().await;
    let paths = backup_paths()?;
    tokio::task::spawn_blocking(move || create_encrypted_backup(paths, passphrase))
        .await
        .map_err(|_| backup_unavailable())?
}

pub(crate) async fn download_backup(
    Extension(role): Extension<ActorRole>,
    Path(backup_id): Path<String>,
) -> Result<Response<Body>, ApiError> {
    require_admin(role)?;
    download_backup_from(backup_id, backup_directory()?).await
}

async fn download_backup_from(
    backup_id: String,
    directory: PathBuf,
) -> Result<Response<Body>, ApiError> {
    let id = Uuid::parse_str(&backup_id).map_err(|_| ApiError::not_found("backup not found"))?;
    let path = backup_file_path(&directory, id);
    let file = File::open(&path)
        .await
        .map_err(|_| ApiError::not_found("backup not found"))?;
    let filename = format!("lxcup-backup-{id}.tar.age");
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
        .map_err(|_| backup_unavailable())?;
    Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_DISPOSITION, disposition)
        .body(Body::from_stream(ReaderStream::new(file)))
        .map_err(|_| backup_unavailable())
}

fn require_admin(role: ActorRole) -> Result<(), ApiError> {
    if role == ActorRole::Admin {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "admin_required",
            "administrator access is required for backups",
        ))
    }
}

fn backup_paths() -> Result<BackupPaths, ApiError> {
    configured_backup_paths(
        env::var("DATABASE_URL").ok(),
        env::var("LXCUP_BACKUP_RETENTION_DAYS").ok(),
        env::var_os("LXCUP_BACKUP_DIR").map(PathBuf::from),
        env::var_os("LXCUP_SECRET_STORE_DIR").map(PathBuf::from),
    )
}

fn configured_backup_paths(
    database_url: Option<String>,
    retention: Option<String>,
    directory: Option<PathBuf>,
    secrets: Option<PathBuf>,
) -> Result<BackupPaths, ApiError> {
    let database_url = database_url
        .and_then(|value| url::Url::parse(&value).ok())
        .filter(|url| matches!(url.scheme(), "postgres" | "postgresql"))
        .ok_or_else(|| {
            ApiError::dependency(
                "backup_not_configured",
                "configure DATABASE_URL for the reachable PostgreSQL database",
            )
        })?;
    let retention_days = retention
        .map(|value| value.parse::<u32>())
        .transpose()
        .map_err(|_| {
            ApiError::dependency(
                "backup_not_configured",
                "LXCUP_BACKUP_RETENTION_DAYS must be between 1 and 3650",
            )
        })?
        .unwrap_or(DEFAULT_RETENTION_DAYS);
    if !(1..=MAX_RETENTION_DAYS).contains(&retention_days) {
        return Err(ApiError::dependency(
            "backup_not_configured",
            "LXCUP_BACKUP_RETENTION_DAYS must be between 1 and 3650",
        ));
    }
    Ok(BackupPaths {
        directory: directory.unwrap_or_else(|| PathBuf::from(DEFAULT_BACKUP_DIRECTORY)),
        secrets: secrets.unwrap_or_else(|| PathBuf::from(DEFAULT_SECRET_DIRECTORY)),
        retention_days,
        database_url,
    })
}

fn backup_directory() -> Result<PathBuf, ApiError> {
    Ok(env::var_os("LXCUP_BACKUP_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_BACKUP_DIRECTORY)))
}

fn create_encrypted_backup(
    paths: BackupPaths,
    passphrase: SecretString,
) -> Result<BackupInfo, ApiError> {
    create_encrypted_backup_with(paths, passphrase, &SystemBackupToolchain)
}

fn create_encrypted_backup_with(
    paths: BackupPaths,
    passphrase: SecretString,
    tools: &impl BackupToolchain,
) -> Result<BackupInfo, ApiError> {
    fs::create_dir_all(&paths.directory).map_err(|_| backup_unavailable())?;
    restrict_directory(&paths.directory)?;
    let workspace = PrivateDirectory::create(
        paths
            .directory
            .join(format!(".staging-{}", Uuid::new_v4().simple())),
    )?;
    let dump = workspace.0.join("postgres.dump");
    tools.dump_database(&paths.database_url, &dump)?;
    copy_encrypted_secrets(&paths.secrets, &workspace.0.join("secrets"))?;
    write_backup_templates(&workspace.0.join("config"))?;
    let archive = workspace.0.join("backup.tar");
    tools.create_archive(&workspace.0, &archive)?;
    let id = Uuid::new_v4();
    let partial = paths.directory.join(format!(".lxcup-{id}.partial"));
    let output = backup_file_path(&paths.directory, id);
    let encrypted = tools.encrypt(&archive, &partial, passphrase);
    if let Err(error) = encrypted {
        let _ = fs::remove_file(&partial);
        return Err(error);
    }
    fs::rename(&partial, &output).map_err(|_| {
        let _ = fs::remove_file(&partial);
        backup_unavailable()
    })?;
    if prune_expired_backups(&paths.directory, paths.retention_days).is_err() {
        tracing::warn!("encrypted backup succeeded but retention cleanup failed");
    }
    info_for_file(id, &output)
}

fn write_backup_templates(directory: &FsPath) -> Result<(), ApiError> {
    fs::create_dir(directory).map_err(|_| backup_unavailable())?;
    restrict_directory(directory)?;
    fs::write(
        directory.join("compose.yaml"),
        include_str!("../../../compose.yaml"),
    )
    .map_err(|_| backup_unavailable())?;
    fs::write(
        directory.join("compose.prod.yaml"),
        include_str!("../../../deploy/compose.prod.yaml"),
    )
    .map_err(|_| backup_unavailable())?;
    fs::write(
        directory.join(".env.example"),
        include_str!("../../../.env.example"),
    )
    .map_err(|_| backup_unavailable())?;
    Ok(())
}

fn dump_database(database_url: &url::Url, output: &FsPath) -> Result<(), ApiError> {
    let mut command = database_dump_command(database_url, output)?;
    run_command(&mut command).map_err(|_| {
        ApiError::dependency(
            "backup_database_failed",
            "database export failed; verify database access and that pg_dump is installed",
        )
    })
}

fn database_dump_command(database_url: &url::Url, output: &FsPath) -> Result<Command, ApiError> {
    let host = database_url.host_str().ok_or_else(backup_unavailable)?;
    let database = database_url.path().trim_start_matches('/');
    let username = percent_decode_str(database_url.username())
        .decode_utf8()
        .map_err(|_| backup_unavailable())?;
    if host.is_empty() || database.is_empty() || username.is_empty() {
        return Err(backup_unavailable());
    }
    let mut command = backup_command("pg_dump");
    command
        .arg("--no-password")
        .arg("--format=custom")
        .arg("--no-owner")
        .arg("--file")
        .arg(output)
        .env("PGHOST", host)
        .env("PGPORT", database_url.port().unwrap_or(5432).to_string())
        .env("PGDATABASE", database)
        .env("PGUSER", username.as_ref())
        .env_remove("PGPASSWORD")
        .env_remove("PGPASSFILE")
        .env_remove("PGSERVICE")
        .env_remove("PGSERVICEFILE")
        .env_remove("PGOPTIONS")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(password) = database_url.password() {
        let decoded = percent_decode_str(password)
            .decode_utf8()
            .map_err(|_| backup_unavailable())?;
        command.env("PGPASSWORD", decoded.as_ref());
    }
    for (key, environment) in [
        ("sslmode", "PGSSLMODE"),
        ("sslrootcert", "PGSSLROOTCERT"),
        ("sslcert", "PGSSLCERT"),
        ("sslkey", "PGSSLKEY"),
    ] {
        if let Some(value) = database_url
            .query_pairs()
            .find_map(|(query_key, query_value)| {
                (query_key == key).then(|| query_value.into_owned())
            })
        {
            command.env(environment, value);
        }
    }
    Ok(command)
}

fn copy_encrypted_secrets(source: &FsPath, destination: &FsPath) -> Result<(), ApiError> {
    let source = fs::canonicalize(source).map_err(|_| backup_unavailable())?;
    fs::create_dir(destination).map_err(|_| backup_unavailable())?;
    restrict_directory(destination)?;
    let mut metadata_ids = HashSet::new();
    let mut ciphertext_ids = HashSet::new();
    for entry in fs::read_dir(source).map_err(|_| backup_unavailable())? {
        let entry = entry.map_err(|_| backup_unavailable())?;
        let file_type = entry.file_type().map_err(|_| backup_unavailable())?;
        if !file_type.is_file() {
            return Err(backup_unavailable());
        }
        let path = entry.path();
        let extension = path.extension().and_then(|value| value.to_str());
        let id = path
            .file_stem()
            .and_then(|value| value.to_str())
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(backup_unavailable)?;
        let ids = match extension {
            Some("json") => &mut metadata_ids,
            Some("enc") => &mut ciphertext_ids,
            _ => return Err(backup_unavailable()),
        };
        if !ids.insert(id) {
            return Err(backup_unavailable());
        }
        let destination_path = destination.join(entry.file_name());
        fs::copy(path, destination_path).map_err(|_| backup_unavailable())?;
    }
    if metadata_ids != ciphertext_ids {
        return Err(backup_unavailable());
    }
    Ok(())
}

fn list_backup_files(directory: &FsPath) -> Result<Vec<BackupInfo>, ApiError> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut backups = fs::read_dir(directory)
        .map_err(|_| backup_unavailable())?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            if !file_type.is_file() {
                return None;
            }
            let id = parse_backup_id(&entry.file_name().to_string_lossy())?;
            info_for_file(id, &entry.path()).ok()
        })
        .collect::<Vec<_>>();
    backups.sort_by_key(|backup| std::cmp::Reverse(backup.created_at));
    backups.truncate(MAX_LISTED_BACKUPS);
    Ok(backups)
}

fn prune_expired_backups(directory: &FsPath, retention_days: u32) -> Result<(), ApiError> {
    let cutoff = SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(
            u64::from(retention_days) * 86_400,
        ))
        .ok_or_else(backup_unavailable)?;
    for entry in fs::read_dir(directory).map_err(|_| backup_unavailable())? {
        let entry = entry.map_err(|_| backup_unavailable())?;
        if !entry
            .file_type()
            .map_err(|_| backup_unavailable())?
            .is_file()
            || !is_backup_filename(&entry.file_name().to_string_lossy())
        {
            continue;
        }
        let metadata = entry.metadata().map_err(|_| backup_unavailable())?;
        if metadata.modified().is_ok_and(|modified| modified < cutoff) {
            fs::remove_file(entry.path()).map_err(|_| backup_unavailable())?;
        }
    }
    Ok(())
}

fn info_for_file(id: Uuid, path: &FsPath) -> Result<BackupInfo, ApiError> {
    let metadata = fs::metadata(path).map_err(|_| ApiError::not_found("backup not found"))?;
    if !metadata.is_file() {
        return Err(ApiError::not_found("backup not found"));
    }
    let modified = metadata.modified().map_err(|_| backup_unavailable())?;
    Ok(BackupInfo {
        id,
        created_at: DateTime::<Utc>::from(modified),
        size_bytes: metadata.len(),
    })
}

fn backup_file_path(directory: &FsPath, id: Uuid) -> PathBuf {
    directory.join(format!("lxcup-{id}.tar.age"))
}

fn parse_backup_id(filename: &str) -> Option<Uuid> {
    filename
        .strip_prefix("lxcup-")?
        .strip_suffix(".tar.age")?
        .parse::<Uuid>()
        .ok()
}

fn is_backup_filename(filename: &str) -> bool {
    parse_backup_id(filename).is_some()
}

pub(crate) fn validate_backup_passphrase(passphrase: &str) -> Result<(), ApiError> {
    let length = passphrase.chars().count();
    if length < MIN_BACKUP_PASSPHRASE_CHARACTERS || passphrase.len() > MAX_BACKUP_PASSPHRASE_BYTES {
        return Err(ApiError::bad_request(
            "backup_passphrase_invalid",
            "backup passphrase must contain at least 12 characters and no more than 1024 bytes",
        ));
    }
    Ok(())
}

fn encrypt_archive(archive: &FsPath, output: &FsPath, passphrase: SecretString) -> io::Result<()> {
    let encryptor = Encryptor::with_user_passphrase(passphrase);
    let output = fs::File::create(output)?;
    let mut writer = encryptor
        .wrap_output(output)
        .map_err(|error| io::Error::other(error.to_string()))?;
    let mut input = fs::File::open(archive)?;
    io::copy(&mut input, &mut writer)?;
    writer
        .finish()
        .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(())
}

fn run_command(command: &mut Command) -> Result<(), ApiError> {
    let status = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| backup_unavailable())?;
    if status.success() {
        Ok(())
    } else {
        Err(backup_unavailable())
    }
}

fn backup_command(program: &str) -> Command {
    let mut command = Command::new(program);
    command.env_clear();
    if let Some(path) = env::var_os("PATH") {
        command.env("PATH", path);
    }
    command
}

fn restrict_directory(_path: &FsPath) -> Result<(), ApiError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(_path, fs::Permissions::from_mode(0o700))
            .map_err(|_| backup_unavailable())?;
    }
    Ok(())
}

fn backup_unavailable() -> ApiError {
    ApiError::dependency(
        "backup_unavailable",
        "backup could not be created or read; check backup-volume permissions, secret-store access, and available storage",
    )
}

#[cfg(test)]
#[path = "backups_tests.rs"]
mod tests;
