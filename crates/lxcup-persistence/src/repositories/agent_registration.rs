use super::*;

impl AgentRegistrationRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn save(&self, registration: &AgentRegistration) -> Result<(), RepositoryError> {
        let payload = serde_json::to_value(registration).map_err(RepositoryError::Serialization)?;
        sqlx::query(
            "INSERT INTO agent_registrations (id, container_id, agent_id, endpoint, secret_ref, ca_secret_ref, state, payload, last_checked_at, last_error, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) ON CONFLICT (container_id) DO UPDATE SET agent_id = EXCLUDED.agent_id, endpoint = EXCLUDED.endpoint, secret_ref = EXCLUDED.secret_ref, ca_secret_ref = EXCLUDED.ca_secret_ref, state = EXCLUDED.state, payload = EXCLUDED.payload, last_checked_at = EXCLUDED.last_checked_at, last_error = EXCLUDED.last_error, updated_at = EXCLUDED.updated_at",
        )
        .bind(registration.id.as_uuid())
        .bind(registration.container_id.value() as i64)
        .bind(&registration.agent_id)
        .bind(&registration.endpoint)
        .bind(registration.secret_ref.as_uuid())
        .bind(registration.ca_secret_ref.map(SecretId::as_uuid))
        .bind(agent_state_to_db(registration.state))
        .bind(payload)
        .bind(registration.last_checked_at)
        .bind(registration.last_error.as_deref())
        .bind(registration.created_at)
        .bind(registration.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn find_by_container(
        &self,
        container_id: ContainerId,
    ) -> Result<Option<AgentRegistration>, RepositoryError> {
        let row = sqlx::query("SELECT payload FROM agent_registrations WHERE container_id = $1")
            .bind(container_id.value() as i64)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| {
            serde_json::from_value(row.try_get("payload")?).map_err(|_| {
                RepositoryError::InvalidValue {
                    field: "agent registration payload",
                }
            })
        })
        .transpose()
    }

    pub async fn list(&self) -> Result<Vec<AgentRegistration>, RepositoryError> {
        let rows = sqlx::query("SELECT payload FROM agent_registrations ORDER BY container_id")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_value(row.try_get("payload")?).map_err(|_| {
                    RepositoryError::InvalidValue {
                        field: "agent registration payload",
                    }
                })
            })
            .collect()
    }
}
