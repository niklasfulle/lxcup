use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::{Database, RepositoryError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthUserRole {
    Admin,
    User,
}

impl AuthUserRole {
    fn as_db(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::User => "user",
        }
    }

    fn from_db(value: &str) -> Result<Self, RepositoryError> {
        match value {
            "admin" => Ok(Self::Admin),
            "user" => Ok(Self::User),
            _ => Err(RepositoryError::InvalidValue { field: "user role" }),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthUser {
    pub id: Uuid,
    pub username: String,
    pub password_hash: String,
    pub role: AuthUserRole,
    pub must_change_password: bool,
    pub disabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistedAuthSession {
    pub id: Uuid,
    pub user: AuthUser,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthAuditEvent {
    pub id: Uuid,
    pub actor_user_id: Option<Uuid>,
    pub actor_username: String,
    pub actor_role: AuthUserRole,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub request_id: Option<Uuid>,
    pub status_code: i16,
    pub details: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default)]
pub struct AuthAuditFilters {
    pub limit: i64,
    pub offset: i64,
    pub actor_username: Option<String>,
    pub action: Option<String>,
    pub resource: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct AuthAuditPage {
    pub events: Vec<AuthAuditEvent>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[derive(FromRow)]
struct AuthUserRow {
    id: Uuid,
    username: String,
    password_hash: String,
    role: String,
    must_change_password: bool,
    disabled: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct AuthSessionRow {
    session_id: Uuid,
    expires_at: DateTime<Utc>,
    user_id: Uuid,
    username: String,
    password_hash: String,
    role: String,
    must_change_password: bool,
    disabled: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl AuthSessionRow {
    fn into_session(self) -> Result<PersistedAuthSession, RepositoryError> {
        Ok(PersistedAuthSession {
            id: self.session_id,
            expires_at: self.expires_at,
            user: AuthUser::try_from(AuthUserRow {
                id: self.user_id,
                username: self.username,
                password_hash: self.password_hash,
                role: self.role,
                must_change_password: self.must_change_password,
                disabled: self.disabled,
                created_at: self.created_at,
                updated_at: self.updated_at,
            })?,
        })
    }
}

#[derive(Clone)]
pub struct AuthRepository {
    pool: PgPool,
}

impl AuthRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn user_by_username(
        &self,
        username: &str,
    ) -> Result<Option<AuthUser>, RepositoryError> {
        let row = sqlx::query_as::<_, AuthUserRow>(
            "SELECT id, username, password_hash, role, must_change_password, disabled, created_at, updated_at FROM auth_users WHERE username = $1",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await?;
        row.map(AuthUser::try_from).transpose()
    }

    pub async fn user_by_id(&self, user_id: Uuid) -> Result<Option<AuthUser>, RepositoryError> {
        let row = sqlx::query_as::<_, AuthUserRow>(
            "SELECT id, username, password_hash, role, must_change_password, disabled, created_at, updated_at FROM auth_users WHERE id = $1",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(AuthUser::try_from).transpose()
    }

    pub async fn list_users(&self) -> Result<Vec<AuthUser>, RepositoryError> {
        let rows = sqlx::query_as::<_, AuthUserRow>(
            "SELECT id, username, password_hash, role, must_change_password, disabled, created_at, updated_at FROM auth_users ORDER BY username",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(AuthUser::try_from).collect()
    }

    /// Creates the bootstrap administrator only when the table is still empty.
    /// The transaction-scoped advisory lock makes concurrent controller starts safe.
    pub async fn create_initial_admin(&self, password_hash: &str) -> Result<bool, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(718244113)")
            .execute(&mut *transaction)
            .await?;
        let has_users: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM auth_users)")
            .fetch_one(&mut *transaction)
            .await?;
        if has_users {
            transaction.commit().await?;
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO auth_users (id, username, password_hash, role, must_change_password) VALUES ($1, 'admin', $2, 'admin', TRUE)",
        )
        .bind(Uuid::new_v4())
        .bind(password_hash)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(true)
    }

    pub async fn create_user(
        &self,
        username: &str,
        password_hash: &str,
        role: AuthUserRole,
        must_change_password: bool,
    ) -> Result<AuthUser, RepositoryError> {
        let result = sqlx::query_as::<_, AuthUserRow>(
            "INSERT INTO auth_users (id, username, password_hash, role, must_change_password) VALUES ($1, $2, $3, $4, $5) RETURNING id, username, password_hash, role, must_change_password, disabled, created_at, updated_at",
        )
        .bind(Uuid::new_v4())
        .bind(username)
        .bind(password_hash)
        .bind(role.as_db())
        .bind(must_change_password)
        .fetch_one(&self.pool)
        .await;
        let row = match result {
            Ok(row) => row,
            Err(sqlx::Error::Database(error))
                if error.code().as_deref() == Some("23505")
                    && error.constraint() == Some("auth_users_username_key") =>
            {
                return Err(RepositoryError::UsernameExists);
            }
            Err(error) => return Err(error.into()),
        };
        AuthUser::try_from(row)
    }

    pub async fn update_user(
        &self,
        user_id: Uuid,
        role: AuthUserRole,
        disabled: bool,
    ) -> Result<Option<AuthUser>, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        lock_user_administration(&mut transaction).await?;
        let existing: Option<String> =
            sqlx::query_scalar("SELECT role FROM auth_users WHERE id = $1 FOR UPDATE")
                .bind(user_id)
                .fetch_optional(&mut *transaction)
                .await?;
        let Some(existing_role) = existing else {
            transaction.commit().await?;
            return Ok(None);
        };
        if existing_role == "admin" && (role != AuthUserRole::Admin || disabled) {
            let active_admins: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM auth_users WHERE role = 'admin' AND disabled = FALSE",
            )
            .fetch_one(&mut *transaction)
            .await?;
            if active_admins <= 1 {
                return Err(RepositoryError::LastAdmin);
            }
        }
        let row = sqlx::query_as::<_, AuthUserRow>(
            "UPDATE auth_users SET role = $2, disabled = $3, updated_at = NOW() WHERE id = $1 RETURNING id, username, password_hash, role, must_change_password, disabled, created_at, updated_at",
        )
        .bind(user_id)
        .bind(role.as_db())
        .bind(disabled)
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE auth_sessions SET revoked_at = NOW() WHERE user_id = $1 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        AuthUser::try_from(row).map(Some)
    }

    pub async fn delete_user(&self, user_id: Uuid) -> Result<bool, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        lock_user_administration(&mut transaction).await?;
        let role: Option<String> =
            sqlx::query_scalar("SELECT role FROM auth_users WHERE id = $1 FOR UPDATE")
                .bind(user_id)
                .fetch_optional(&mut *transaction)
                .await?;
        let Some(role) = role else {
            transaction.commit().await?;
            return Ok(false);
        };
        if role == "admin" {
            let active_admins: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM auth_users WHERE role = 'admin' AND disabled = FALSE",
            )
            .fetch_one(&mut *transaction)
            .await?;
            if active_admins <= 1 {
                return Err(RepositoryError::LastAdmin);
            }
        }
        sqlx::query("DELETE FROM auth_users WHERE id = $1")
            .bind(user_id)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(true)
    }

    pub async fn reset_user_password(
        &self,
        user_id: Uuid,
        password_hash: &str,
    ) -> Result<bool, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let updated = sqlx::query("UPDATE auth_users SET password_hash = $2, must_change_password = TRUE, updated_at = NOW() WHERE id = $1 AND disabled = FALSE")
            .bind(user_id)
            .bind(password_hash)
            .execute(&mut *transaction)
            .await?
            .rows_affected() == 1;
        if updated {
            sqlx::query("UPDATE auth_sessions SET revoked_at = NOW() WHERE user_id = $1 AND revoked_at IS NULL")
                .bind(user_id)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(updated)
    }

    pub async fn save_session(
        &self,
        user_id: Uuid,
        token_hash: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<Uuid, RepositoryError> {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO auth_sessions (id, user_id, token_hash, expires_at) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(user_id)
            .bind(token_hash)
            .bind(expires_at)
            .execute(&self.pool)
            .await?;
        Ok(id)
    }

    pub async fn session_by_token_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<PersistedAuthSession>, RepositoryError> {
        let row = sqlx::query_as::<_, AuthSessionRow>(
            "SELECT s.id AS session_id, s.expires_at, u.id AS user_id, u.username, u.password_hash, u.role, u.must_change_password, u.disabled, u.created_at, u.updated_at FROM auth_sessions s JOIN auth_users u ON u.id = s.user_id WHERE s.token_hash = $1 AND s.revoked_at IS NULL AND s.expires_at > NOW() AND u.disabled = FALSE",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        row.map(AuthSessionRow::into_session).transpose()
    }

    pub async fn revoke_session(&self, token_hash: &str) -> Result<(), RepositoryError> {
        sqlx::query("UPDATE auth_sessions SET revoked_at = NOW() WHERE token_hash = $1 AND revoked_at IS NULL")
            .bind(token_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn change_password(
        &self,
        user_id: Uuid,
        password_hash: &str,
        current_session_hash: &str,
    ) -> Result<bool, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let updated = sqlx::query("UPDATE auth_users SET password_hash = $2, must_change_password = FALSE, updated_at = NOW() WHERE id = $1 AND disabled = FALSE")
            .bind(user_id)
            .bind(password_hash)
            .execute(&mut *transaction)
            .await?
            .rows_affected()
            == 1;
        if updated {
            sqlx::query("UPDATE auth_sessions SET revoked_at = NOW() WHERE user_id = $1 AND token_hash <> $2 AND revoked_at IS NULL")
                .bind(user_id)
                .bind(current_session_hash)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(updated)
    }

    pub async fn append_audit_event(&self, event: &AuthAuditEvent) -> Result<(), RepositoryError> {
        sqlx::query("INSERT INTO user_audit_events (id, actor_user_id, actor_username, actor_role, action, resource_type, resource_id, request_id, status_code, details, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)")
            .bind(event.id)
            .bind(event.actor_user_id)
            .bind(&event.actor_username)
            .bind(event.actor_role.as_db())
            .bind(&event.action)
            .bind(&event.resource_type)
            .bind(&event.resource_id)
            .bind(event.request_id)
            .bind(event.status_code)
            .bind(&event.details)
            .bind(event.created_at)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn update_audit_event_status(
        &self,
        event_id: Uuid,
        status_code: i16,
        state: &str,
    ) -> Result<bool, RepositoryError> {
        let updated = sqlx::query("UPDATE user_audit_events SET status_code = $2, details = jsonb_set(details, '{state}', to_jsonb($3::text), TRUE) WHERE id = $1")
            .bind(event_id)
            .bind(status_code)
            .bind(state)
            .execute(&self.pool)
            .await?
            .rows_affected()
            == 1;
        Ok(updated)
    }

    pub async fn list_audit_events(
        &self,
        filters: &AuthAuditFilters,
    ) -> Result<AuthAuditPage, RepositoryError> {
        let limit = filters.limit.clamp(1, 100);
        let offset = filters.offset.max(0);
        let username = nonempty_filter(filters.actor_username.as_deref());
        let action = nonempty_filter(filters.action.as_deref());
        let resource = nonempty_filter(filters.resource.as_deref());
        let total: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM user_audit_events WHERE ($1::text IS NULL OR actor_username ILIKE '%' || $1 || '%') AND ($2::text IS NULL OR action ILIKE '%' || $2 || '%') AND ($3::text IS NULL OR resource_type ILIKE '%' || $3 || '%' OR COALESCE(resource_id, '') ILIKE '%' || $3 || '%') AND ($4::timestamptz IS NULL OR created_at >= $4) AND ($5::timestamptz IS NULL OR created_at < $5)",
        )
        .bind(&username)
        .bind(&action)
        .bind(&resource)
        .bind(filters.since)
        .bind(filters.until)
        .fetch_one(&self.pool)
        .await?;
        let rows = sqlx::query_as::<_, AuditEventRow>(
            "SELECT id, actor_user_id, actor_username, actor_role, action, resource_type, resource_id, request_id, status_code, details, created_at FROM user_audit_events WHERE ($1::text IS NULL OR actor_username ILIKE '%' || $1 || '%') AND ($2::text IS NULL OR action ILIKE '%' || $2 || '%') AND ($3::text IS NULL OR resource_type ILIKE '%' || $3 || '%' OR COALESCE(resource_id, '') ILIKE '%' || $3 || '%') AND ($4::timestamptz IS NULL OR created_at >= $4) AND ($5::timestamptz IS NULL OR created_at < $5) ORDER BY created_at DESC LIMIT $6 OFFSET $7",
        )
        .bind(&username)
        .bind(&action)
        .bind(&resource)
        .bind(filters.since)
        .bind(filters.until)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(AuthAuditPage {
            events: rows
                .into_iter()
                .map(AuthAuditEvent::try_from)
                .collect::<Result<_, _>>()?,
            total,
            limit,
            offset,
        })
    }
}

fn nonempty_filter(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[derive(FromRow)]
struct AuditEventRow {
    id: Uuid,
    actor_user_id: Option<Uuid>,
    actor_username: String,
    actor_role: String,
    action: String,
    resource_type: String,
    resource_id: Option<String>,
    request_id: Option<Uuid>,
    status_code: i16,
    details: Value,
    created_at: DateTime<Utc>,
}

impl TryFrom<AuditEventRow> for AuthAuditEvent {
    type Error = RepositoryError;

    fn try_from(row: AuditEventRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            actor_user_id: row.actor_user_id,
            actor_username: row.actor_username,
            actor_role: AuthUserRole::from_db(&row.actor_role)?,
            action: row.action,
            resource_type: row.resource_type,
            resource_id: row.resource_id,
            request_id: row.request_id,
            status_code: row.status_code,
            details: row.details,
            created_at: row.created_at,
        })
    }
}

async fn lock_user_administration(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), RepositoryError> {
    sqlx::query("SELECT pg_advisory_xact_lock(718244114)")
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

impl TryFrom<AuthUserRow> for AuthUser {
    type Error = RepositoryError;

    fn try_from(row: AuthUserRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            username: row.username,
            password_hash: row.password_hash,
            role: AuthUserRole::from_db(&row.role)?,
            must_change_password: row.must_change_password,
            disabled: row.disabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}
