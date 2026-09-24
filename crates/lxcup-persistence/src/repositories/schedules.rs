use super::{Database, RepositoryError, ScheduleRepository};
use chrono::{DateTime, Utc};
use lxcup_core::{JobSchedule, ScheduleFrequency, TargetId};
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

impl ScheduleRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn save(&self, schedule: &JobSchedule) -> Result<(), RepositoryError> {
        let every_minutes = match schedule.frequency {
            ScheduleFrequency::EveryMinutes(value) => {
                i32::try_from(value).map_err(|_| RepositoryError::InvalidValue {
                    field: "schedule interval",
                })?
            }
        };
        let target_ids = schedule
            .target_ids
            .iter()
            .map(|target_id| target_id.as_uuid())
            .collect::<Vec<Uuid>>();
        sqlx::query(
            "INSERT INTO schedules (id, operation, timezone, target_ids, every_minutes, enabled, threshold, policy_id, last_run_at, next_run_at, last_error, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NOW())
             ON CONFLICT (id) DO UPDATE SET operation = EXCLUDED.operation, timezone = EXCLUDED.timezone,
               target_ids = EXCLUDED.target_ids, every_minutes = EXCLUDED.every_minutes,
               enabled = EXCLUDED.enabled, threshold = EXCLUDED.threshold, policy_id = EXCLUDED.policy_id, last_run_at = EXCLUDED.last_run_at,
               next_run_at = EXCLUDED.next_run_at, last_error = EXCLUDED.last_error, updated_at = NOW()",
        )
        .bind(&schedule.id)
        .bind(&schedule.operation)
        .bind(&schedule.timezone)
        .bind(target_ids)
        .bind(every_minutes)
        .bind(schedule.enabled)
        .bind(
            schedule
                .threshold
                .map(serde_json::to_value)
                .transpose()
                .map_err(RepositoryError::Serialization)?,
        )
        .bind(&schedule.policy_id)
        .bind(schedule.last_run_at)
        .bind(schedule.next_run_at)
        .bind(&schedule.last_error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<JobSchedule>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, operation, timezone, target_ids, every_minutes, enabled, threshold, policy_id, last_run_at, next_run_at, last_error
             FROM schedules ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(schedule_from_row).collect()
    }

    pub async fn update(&self, schedule: &JobSchedule) -> Result<(), RepositoryError> {
        self.save(schedule).await
    }
}

fn schedule_from_row(row: sqlx::postgres::PgRow) -> Result<JobSchedule, RepositoryError> {
    let target_ids = row
        .try_get::<Vec<Uuid>, _>("target_ids")?
        .into_iter()
        .map(TargetId::from_uuid)
        .collect();
    let every_minutes = row.try_get::<i32, _>("every_minutes")?;
    if every_minutes <= 0 {
        return Err(RepositoryError::InvalidValue {
            field: "schedule interval",
        });
    }
    Ok(JobSchedule {
        id: row.try_get("id")?,
        operation: row.try_get("operation")?,
        timezone: row.try_get("timezone")?,
        target_ids,
        frequency: ScheduleFrequency::EveryMinutes(u32::try_from(every_minutes).map_err(|_| {
            RepositoryError::InvalidValue {
                field: "schedule interval",
            }
        })?),
        enabled: row.try_get("enabled")?,
        threshold: deserialize_threshold(row.try_get("threshold")?)?,
        policy_id: row.try_get("policy_id")?,
        last_run_at: row.try_get::<Option<DateTime<Utc>>, _>("last_run_at")?,
        next_run_at: row.try_get("next_run_at")?,
        last_error: row.try_get("last_error")?,
    })
}

fn deserialize_threshold(
    value: Option<Value>,
) -> Result<Option<lxcup_core::ThresholdRule>, RepositoryError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    serde_json::from_value(value)
        .map(Some)
        .map_err(|_| RepositoryError::InvalidValue {
            field: "schedule threshold",
        })
}
