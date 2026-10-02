use super::*;
use age::{Decryptor, secrecy::SecretString};
use axum::{Extension, Json, extract::State};
use lxcup_core::ActorRole;
use std::io::Read;

struct FakeToolchain {
    fail_at: Option<&'static str>,
}

impl BackupToolchain for FakeToolchain {
    fn dump_database(&self, _database_url: &url::Url, output: &FsPath) -> Result<(), ApiError> {
        if self.fail_at == Some("dump") {
            return Err(backup_unavailable());
        }
        fs::write(output, b"database dump").map_err(|_| backup_unavailable())
    }

    fn create_archive(&self, workspace: &FsPath, output: &FsPath) -> Result<(), ApiError> {
        if self.fail_at == Some("archive") {
            return Err(backup_unavailable());
        }
        assert!(workspace.join("postgres.dump").exists());
        assert!(workspace.join("secrets").is_dir());
        assert!(workspace.join("config/compose.yaml").is_file());
        fs::write(output, b"complete archive").map_err(|_| backup_unavailable())
    }

    fn encrypt(
        &self,
        archive: &FsPath,
        output: &FsPath,
        _passphrase: SecretString,
    ) -> Result<(), ApiError> {
        assert!(archive.exists());
        fs::write(output, b"partial encrypted archive").map_err(|_| backup_unavailable())?;
        if self.fail_at == Some("encrypt") {
            return Err(backup_unavailable());
        }
        fs::write(output, b"age-encrypted archive").map_err(|_| backup_unavailable())
    }
}

#[test]
fn backup_passphrase_must_be_long_enough_and_bounded() {
    assert!(validate_backup_passphrase("correct horse battery staple").is_ok());
    assert_eq!(
        validate_backup_passphrase("short").unwrap_err().code,
        "backup_passphrase_invalid"
    );
    assert_eq!(
        validate_backup_passphrase(&"x".repeat(MAX_BACKUP_PASSPHRASE_BYTES + 1))
            .unwrap_err()
            .code,
        "backup_passphrase_invalid"
    );
    assert!(validate_backup_passphrase(&"ä".repeat(12)).is_ok());
}

#[test]
fn backup_id_parser_rejects_paths_and_unexpected_files() {
    let id = Uuid::new_v4();
    let filename = format!("lxcup-{id}.tar.age");
    assert_eq!(parse_backup_id(&filename), Some(id));
    assert!(!is_backup_filename(&format!("../{filename}")));
    assert!(!is_backup_filename(&format!("{filename}.partial")));
}

#[test]
fn backup_configuration_requires_supported_database_and_bounded_retention() {
    let paths = configured_backup_paths(
        Some("postgresql://backup:pw@db.example/lxcup".to_owned()),
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(paths.retention_days, DEFAULT_RETENTION_DAYS);
    assert_eq!(paths.database_url.host_str(), Some("db.example"));
    assert_eq!(paths.secrets, PathBuf::from(DEFAULT_SECRET_DIRECTORY));

    for (database, retention) in [
        (Some("https://db/app"), None),
        (Some("postgres://u:p@db/app"), Some("invalid")),
        (Some("postgres://u:p@db/app"), Some("0")),
        (Some("postgres://u:p@db/app"), Some("3651")),
    ] {
        assert!(
            configured_backup_paths(
                database.map(str::to_owned),
                retention.map(str::to_owned),
                None,
                None,
            )
            .is_err()
        );
    }

    let unsupported_database =
        configured_backup_paths(Some("mysql://db/app".to_owned()), None, None, None)
            .err()
            .unwrap();
    assert_eq!(unsupported_database.code, "backup_not_configured");
    assert!(unsupported_database.message.contains("DATABASE_URL"));
}

#[test]
fn database_dump_command_keeps_credentials_out_of_arguments_and_inherited_environment() {
    let mut database_url = url::Url::parse(
        "postgresql://db.example:5434/lxcup?sslmode=verify-full&sslrootcert=%2Fca.crt",
    )
    .unwrap();
    database_url.set_username("backup").unwrap();
    database_url.set_password(Some("fixture-value")).unwrap();
    let command = database_dump_command(&database_url, FsPath::new("database.dump")).unwrap();
    let arguments = command
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(arguments.contains(&"--no-password".to_owned()));
    let password = database_url.password().unwrap();
    assert!(!arguments.iter().any(|argument| argument.contains(password)));
    let environment = command
        .get_envs()
        .filter_map(|(key, value)| {
            value.map(|value| {
                (
                    key.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            })
        })
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(
        environment.get("PGHOST").map(String::as_str),
        Some("db.example")
    );
    assert_eq!(environment.get("PGPORT").map(String::as_str), Some("5434"));
    assert_eq!(
        environment.get("PGPASSWORD").map(String::as_str),
        Some(password)
    );
    assert_eq!(
        environment.get("PGSSLMODE").map(String::as_str),
        Some("verify-full")
    );
    assert!(!environment.contains_key("DATABASE_URL"));
    assert!(!environment.contains_key("LXCUP_SECRET_MASTER_KEY"));
}

#[test]
fn command_runner_handles_success_failure_and_missing_programs() {
    #[cfg(windows)]
    let mut success = {
        let mut command = Command::new("cmd.exe");
        command.args(["/C", "exit", "0"]);
        command
    };
    #[cfg(unix)]
    let mut success = {
        let mut command = Command::new("sh");
        command.args(["-c", "exit 0"]);
        command
    };
    assert!(run_command(&mut success).is_ok());

    #[cfg(windows)]
    let mut failure = {
        let mut command = Command::new("cmd.exe");
        command.args(["/C", "exit", "1"]);
        command
    };
    #[cfg(unix)]
    let mut failure = {
        let mut command = Command::new("sh");
        command.args(["-c", "exit 1"]);
        command
    };
    assert!(run_command(&mut failure).is_err());
    assert!(run_command(&mut backup_command("lxcup-nonexistent-backup-tool")).is_err());
}

#[test]
fn backup_command_does_not_inherit_secret_environment_values() {
    let command = backup_command("age");
    let inherited = command
        .get_envs()
        .map(|(key, _)| key.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(!inherited.iter().any(|key| key.contains("SECRET")));
    assert!(!inherited.iter().any(|key| key == "DATABASE_URL"));
}

#[test]
fn passphrase_encrypted_backup_is_compatible_with_age_decryption() {
    let root = env::temp_dir().join(format!("lxcup-age-passphrase-test-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let archive = root.join("backup.tar");
    let encrypted = root.join("backup.tar.age");
    let passphrase = SecretString::from("correct horse battery staple".to_owned());
    fs::write(&archive, b"backup contents").unwrap();

    encrypt_archive(&archive, &encrypted, passphrase.clone()).unwrap();
    let decryptor = Decryptor::new(fs::File::open(&encrypted).unwrap()).unwrap();
    let identity = age::scrypt::Identity::new(passphrase);
    let mut reader = decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .unwrap();
    let mut decrypted = Vec::new();
    reader.read_to_end(&mut decrypted).unwrap();
    assert_eq!(decrypted, b"backup contents");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backup_creation_writes_encrypted_archive_and_prunes_temporary_files() {
    let (root, paths) = backup_fixture();
    let backup =
        create_encrypted_backup_with(paths, test_passphrase(), &FakeToolchain { fail_at: None })
            .unwrap();
    assert_eq!(backup.size_bytes, b"age-encrypted archive".len() as u64);
    assert_eq!(
        list_backup_files(&root.join("backups")).unwrap(),
        vec![backup.clone()]
    );
    assert_eq!(
        fs::read(backup_file_path(&root.join("backups"), backup.id)).unwrap(),
        b"age-encrypted archive"
    );
    assert!(fs::read_dir(root.join("backups")).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".staging-")
    }));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backup_creation_cleans_staging_and_partial_output_after_each_failure() {
    for fail_at in ["dump", "archive", "encrypt"] {
        let (root, paths) = backup_fixture();
        assert!(
            create_encrypted_backup_with(
                paths,
                test_passphrase(),
                &FakeToolchain {
                    fail_at: Some(fail_at)
                }
            )
            .is_err()
        );
        let backups = root.join("backups");
        assert!(list_backup_files(&backups).unwrap().is_empty());
        assert!(fs::read_dir(backups).unwrap().next().is_none());
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn backup_listing_includes_only_generated_regular_files() {
    let directory = env::temp_dir().join(format!("lxcup-backups-test-{}", Uuid::new_v4()));
    fs::create_dir_all(&directory).unwrap();
    let id = Uuid::new_v4();
    fs::write(backup_file_path(&directory, id), b"encrypted archive").unwrap();
    fs::write(
        directory.join("lxcup-not-a-uuid.tar.age"),
        b"not a generated backup",
    )
    .unwrap();
    fs::write(directory.join(".lxcup-partial.partial"), b"incomplete").unwrap();

    let backups = list_backup_files(&directory).unwrap();
    assert_eq!(backups.len(), 1);
    assert_eq!(backups[0].id, id);
    assert_eq!(backups[0].size_bytes, b"encrypted archive".len() as u64);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn retention_pruning_removes_only_expired_generated_backups() {
    let directory = env::temp_dir().join(format!("lxcup-retention-test-{}", Uuid::new_v4()));
    fs::create_dir_all(&directory).unwrap();
    let expired = backup_file_path(&directory, Uuid::new_v4());
    let current = backup_file_path(&directory, Uuid::new_v4());
    let unrelated = directory.join("operator-notes.txt");
    fs::write(&expired, b"expired").unwrap();
    fs::write(&current, b"current").unwrap();
    fs::write(&unrelated, b"keep").unwrap();
    let old_time = SystemTime::now() - std::time::Duration::from_secs(2 * 86_400);
    fs::File::options()
        .write(true)
        .open(&expired)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(old_time))
        .unwrap();

    prune_expired_backups(&directory, 1).unwrap();
    assert!(!expired.exists());
    assert!(current.exists());
    assert!(unrelated.exists());
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn backup_creation_requires_admin_and_explicit_confirmation() {
    let forbidden = create_backup(
        State(ApiState::new()),
        Extension(ActorRole::Operator),
        Json(CreateBackupRequest {
            confirmed: true,
            passphrase: "not used by non-admin".to_owned(),
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(forbidden.status, axum::http::StatusCode::FORBIDDEN);

    let unconfirmed = create_backup(
        State(ApiState::new()),
        Extension(ActorRole::Admin),
        Json(CreateBackupRequest {
            confirmed: false,
            passphrase: "unused".to_owned(),
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(unconfirmed.code, "backup_confirmation_required");

    let weak_passphrase = create_backup(
        State(ApiState::new()),
        Extension(ActorRole::Admin),
        Json(CreateBackupRequest {
            confirmed: true,
            passphrase: "too short".to_owned(),
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(weak_passphrase.code, "backup_passphrase_invalid");
}

#[tokio::test]
async fn backup_download_rejects_invalid_ids_before_path_access() {
    let error = download_backup(Extension(ActorRole::Admin), Path("../secret".to_owned()))
        .await
        .unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::NOT_FOUND);
}

#[test]
fn secret_export_refuses_partial_pairs_and_unexpected_files() {
    let root = env::temp_dir().join(format!("lxcup-secret-backup-test-{}", Uuid::new_v4()));
    let copied = root.join("copied");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join(format!("{}.json", Uuid::new_v4())), b"metadata").unwrap();
    assert!(copy_encrypted_secrets(&root, &copied).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn secret_export_copies_only_complete_encrypted_secret_pairs() {
    let root = env::temp_dir().join(format!("lxcup-secret-backup-test-{}", Uuid::new_v4()));
    let source = root.join("secrets");
    let copied = root.join("copied");
    let secret = Uuid::new_v4();
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join(format!("{secret}.json")), b"metadata").unwrap();
    fs::write(source.join(format!("{secret}.enc")), b"encrypted value").unwrap();
    copy_encrypted_secrets(&source, &copied).unwrap();
    assert_eq!(
        fs::read(copied.join(format!("{secret}.enc"))).unwrap(),
        b"encrypted value"
    );
    assert!(copy_encrypted_secrets(&source, &copied).is_err());
    fs::remove_dir_all(root).unwrap();
}

fn backup_fixture() -> (PathBuf, BackupPaths) {
    let root = env::temp_dir().join(format!("lxcup-backup-test-{}", Uuid::new_v4()));
    let directory = root.join("backups");
    let secrets = root.join("secrets");
    fs::create_dir_all(&secrets).unwrap();
    let secret = Uuid::new_v4();
    fs::write(secrets.join(format!("{secret}.json")), b"metadata").unwrap();
    fs::write(secrets.join(format!("{secret}.enc")), b"encrypted value").unwrap();
    let paths = BackupPaths {
        directory,
        secrets,
        retention_days: DEFAULT_RETENTION_DAYS,
        database_url: url::Url::parse("postgres://backup:password@db.example/lxcup").unwrap(),
    };
    (root, paths)
}

fn test_passphrase() -> SecretString {
    SecretString::from("correct horse battery staple".to_owned())
}
