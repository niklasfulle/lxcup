use super::*;
use age::{
    Decryptor,
    secrecy::{ExposeSecret, SecretString},
};
use axum::{Extension, Json, body::to_bytes, extract::State};
use lxcup_core::{ActorRole, ContainerId, SecretValue};
use lxcup_secrets::{CreateSecret, InMemorySecretStore, SecretStore};
use std::{io::Read, sync::Arc};

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
fn backup_configuration_accepts_custom_paths_and_retention_boundaries() {
    let directory = PathBuf::from("/tmp/lxcup-backups");
    let secrets = PathBuf::from("/tmp/lxcup-secrets");
    for retention in ["1", "3650"] {
        let paths = configured_backup_paths(
            Some("postgres://backup:pw@db.example/lxcup".to_owned()),
            Some(retention.to_owned()),
            Some(directory.clone()),
            Some(secrets.clone()),
        )
        .unwrap();
        assert_eq!(paths.directory, directory);
        assert_eq!(paths.secrets, secrets);
        assert_eq!(paths.retention_days, retention.parse::<u32>().unwrap());
    }
    assert_eq!(
        configured_backup_paths(None, None, None, None)
            .err()
            .unwrap()
            .code,
        "backup_not_configured"
    );
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
fn system_toolchain_sanitizes_archive_and_encryption_failures() {
    let root = env::temp_dir().join(format!("lxcup-system-toolchain-test-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let tools = SystemBackupToolchain;

    let archive_error = tools
        .create_archive(&root.join("missing-workspace"), &root.join("backup.tar"))
        .unwrap_err();
    assert_eq!(archive_error.code, "backup_archive_failed");
    assert!(
        !archive_error
            .message
            .contains(root.to_string_lossy().as_ref())
    );

    let encryption_error = tools
        .encrypt(
            &root.join("missing-archive.tar"),
            &root.join("backup.tar.age"),
            test_passphrase(),
        )
        .unwrap_err();
    assert_eq!(encryption_error.code, "backup_encryption_failed");
    assert!(
        !encryption_error
            .message
            .contains(root.to_string_lossy().as_ref())
    );
    fs::remove_dir_all(root).unwrap();
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
fn backup_listing_handles_missing_directory_and_caps_newest_results() {
    let root = env::temp_dir().join(format!("lxcup-backup-list-limit-{}", Uuid::new_v4()));
    let absent = root.join("not-created");
    assert!(list_backup_files(&absent).unwrap().is_empty());
    fs::create_dir_all(&root).unwrap();

    for index in 0..(MAX_LISTED_BACKUPS + 1) {
        let path = backup_file_path(&root, Uuid::new_v4());
        fs::write(&path, index.to_string()).unwrap();
        let modified =
            SystemTime::now() - std::time::Duration::from_secs((MAX_LISTED_BACKUPS - index) as u64);
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
    }
    fs::create_dir(root.join(format!("lxcup-{}.tar.age", Uuid::new_v4()))).unwrap();

    let listed = list_backup_files(&root).unwrap();
    assert_eq!(listed.len(), MAX_LISTED_BACKUPS);
    assert!(
        listed
            .windows(2)
            .all(|pair| pair[0].created_at >= pair[1].created_at)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backup_file_info_rejects_missing_paths_and_directories() {
    let root = env::temp_dir().join(format!("lxcup-backup-info-test-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let missing = info_for_file(Uuid::new_v4(), &root.join("missing.tar.age"))
        .err()
        .unwrap();
    assert_eq!(missing.status, axum::http::StatusCode::NOT_FOUND);

    let directory = root.join("not-a-backup-file");
    fs::create_dir(&directory).unwrap();
    let non_file = info_for_file(Uuid::new_v4(), &directory).err().unwrap();
    assert_eq!(non_file.status, axum::http::StatusCode::NOT_FOUND);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backup_listing_endpoint_authorizes_admin_and_returns_sorted_files() {
    let root = env::temp_dir().join(format!("lxcup-backups-endpoint-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let older = Uuid::new_v4();
    let newer = Uuid::new_v4();
    let older_path = backup_file_path(&root, older);
    let newer_path = backup_file_path(&root, newer);
    fs::write(&older_path, b"older backup").unwrap();
    fs::write(&newer_path, b"newer backup").unwrap();
    let old_time = SystemTime::now() - std::time::Duration::from_secs(60);
    fs::File::options()
        .write(true)
        .open(&older_path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(old_time))
        .unwrap();

    let listed = list_backups_from(root.clone()).await.unwrap();
    assert_eq!(listed.0.data.len(), 2);
    assert_eq!(listed.0.data[0].id, newer);
    assert_eq!(listed.0.data[1].id, older);

    let forbidden = list_backups(Extension(ActorRole::Operator))
        .await
        .unwrap_err();
    assert_eq!(forbidden.status, axum::http::StatusCode::FORBIDDEN);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backup_download_authorizes_before_lookup_and_streams_named_archive() {
    let root = env::temp_dir().join(format!("lxcup-backup-download-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let id = Uuid::new_v4();
    let filename = format!("lxcup-backup-{id}.tar.age");
    let path = backup_file_path(&root, id);
    fs::write(&path, b"encrypted archive bytes").unwrap();

    let forbidden = download_backup(Extension(ActorRole::Operator), Path(id.to_string()))
        .await
        .unwrap_err();
    assert_eq!(forbidden.status, axum::http::StatusCode::FORBIDDEN);

    let invalid = download_backup_from("../secrets".to_owned(), root.clone())
        .await
        .unwrap_err();
    assert_eq!(invalid.status, axum::http::StatusCode::NOT_FOUND);
    let missing = download_backup_from(Uuid::new_v4().to_string(), root.clone())
        .await
        .unwrap_err();
    assert_eq!(missing.status, axum::http::StatusCode::NOT_FOUND);

    let response = download_backup_from(id.to_string(), root.clone())
        .await
        .unwrap();
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap(),
        "application/octet-stream"
    );
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_DISPOSITION)
            .unwrap(),
        &HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")).unwrap()
    );
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        b"encrypted archive bytes".as_slice()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn scheduled_backup_secret_must_be_active_global_backup_passphrase() {
    let store = InMemorySecretStore::default();
    let valid = store
        .create(CreateSecret {
            name: "scheduled backup".to_owned(),
            kind: SecretKind::BackupPassphrase,
            scope: SecretScope::Global,
            value: SecretValue::new("correct horse battery staple").unwrap(),
        })
        .unwrap();
    let generic = store
        .create(CreateSecret {
            name: "generic".to_owned(),
            kind: SecretKind::Generic,
            scope: SecretScope::Global,
            value: SecretValue::new("correct horse battery staple").unwrap(),
        })
        .unwrap();
    let scoped = store
        .create(CreateSecret {
            name: "target backup".to_owned(),
            kind: SecretKind::BackupPassphrase,
            scope: SecretScope::Container(ContainerId::new(1)),
            value: SecretValue::new("correct horse battery staple").unwrap(),
        })
        .unwrap();
    let weak = store
        .create(CreateSecret {
            name: "weak backup".to_owned(),
            kind: SecretKind::BackupPassphrase,
            scope: SecretScope::Global,
            value: SecretValue::new("too short").unwrap(),
        })
        .unwrap();
    let state = ApiState::new().with_secret_store(Arc::new(store.clone()));

    assert_eq!(
        read_backup_passphrase(&state, valid.metadata.id)
            .unwrap()
            .expose_secret(),
        "correct horse battery staple"
    );
    for id in [generic.metadata.id, scoped.metadata.id, SecretId::new()] {
        assert_eq!(
            read_backup_passphrase(&state, id).unwrap_err().code,
            "invalid_backup_secret"
        );
    }
    assert_eq!(
        read_backup_passphrase(&state, weak.metadata.id)
            .unwrap_err()
            .code,
        "backup_passphrase_invalid"
    );
    store.revoke(valid.metadata.id).unwrap();
    assert_eq!(
        read_backup_passphrase(&state, valid.metadata.id)
            .unwrap_err()
            .code,
        "invalid_backup_secret"
    );
}

#[test]
fn scheduled_backup_secret_errors_are_mapped_to_safe_api_errors() {
    let cases = [
        (SecretStoreError::Missing, "invalid_backup_secret", false),
        (SecretStoreError::Invalid, "invalid_backup_secret", false),
        (SecretStoreError::Denied, "invalid_backup_secret", false),
        (
            SecretStoreError::Unavailable,
            "backup_secret_unavailable",
            true,
        ),
    ];
    for (error, expected_code, dependency) in cases {
        let mapped = map_backup_secret_error(error);
        assert_eq!(mapped.code, expected_code);
        assert_eq!(
            mapped.status,
            if dependency {
                axum::http::StatusCode::BAD_GATEWAY
            } else {
                axum::http::StatusCode::BAD_REQUEST
            }
        );
    }
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
fn secret_export_rejects_ciphertext_only_pairs_and_nested_entries() {
    let root = env::temp_dir().join(format!("lxcup-secret-export-errors-{}", Uuid::new_v4()));
    let copied = root.join("copied");
    fs::create_dir_all(&root).unwrap();
    let secret_id = Uuid::new_v4();
    let ciphertext = root.join(format!("{secret_id}.enc"));
    fs::write(&ciphertext, b"ciphertext").unwrap();
    assert!(copy_encrypted_secrets(&root, &copied).is_err());
    fs::remove_file(ciphertext).unwrap();
    fs::create_dir(root.join("nested")).unwrap();
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
