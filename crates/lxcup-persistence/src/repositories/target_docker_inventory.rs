use super::{Database, RepositoryError, TargetDockerInventoryRepository};
use chrono::{DateTime, Utc};
use lxcup_agent::DockerContainerInfo;
use lxcup_core::TargetId;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::HashMap;

const MAX_DOCKER_INVENTORY_EVENTS: usize = 200;
const DOCKER_INVENTORY_EVENT_RETENTION_DAYS: i64 = 180;
const DOCKER_EVENT_PRUNE_BATCH_SIZE: i64 = 100;

#[derive(Clone, Debug)]
pub struct PersistedTargetDockerInventory {
    pub collected_at: DateTime<Utc>,
    pub containers: Vec<DockerContainerInfo>,
    pub events: Vec<DockerInventoryEvent>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DockerInventoryEvent {
    pub observed_at: DateTime<Utc>,
    pub container_id: String,
    pub container_name: String,
    pub kind: DockerInventoryEventKind,
    pub previous_value: Option<String>,
    pub current_value: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DockerInventoryEventKind {
    Added,
    Removed,
    ImageChanged,
    StateChanged,
    HealthChanged,
    RestartCountIncreased,
    OomKilled,
}

pub fn update_docker_inventory_events(
    previous: Option<&PersistedTargetDockerInventory>,
    current: &mut PersistedTargetDockerInventory,
) {
    let mut events = previous.map_or_else(Vec::new, |inventory| inventory.events.clone());
    let previous_containers = previous.map_or(&[][..], |inventory| inventory.containers.as_slice());
    let previous_by_id = previous_containers
        .iter()
        .map(|container| (container.id.as_str(), container))
        .collect::<HashMap<_, _>>();
    let current_by_id = current
        .containers
        .iter()
        .map(|container| (container.id.as_str(), container))
        .collect::<HashMap<_, _>>();

    record_current_container_events(current, &previous_by_id, previous.is_some(), &mut events);
    record_removed_container_events(previous, &current_by_id, current.collected_at, &mut events);

    if events.len() > MAX_DOCKER_INVENTORY_EVENTS {
        events.drain(..events.len() - MAX_DOCKER_INVENTORY_EVENTS);
    }
    current.events = events;
}

fn record_current_container_events(
    current: &PersistedTargetDockerInventory,
    previous_by_id: &HashMap<&str, &DockerContainerInfo>,
    has_previous: bool,
    events: &mut Vec<DockerInventoryEvent>,
) {
    for container in &current.containers {
        let Some(old) = previous_by_id.get(container.id.as_str()) else {
            if has_previous {
                push_event(
                    events,
                    current.collected_at,
                    container,
                    DockerInventoryEventKind::Added,
                    None,
                    Some("present".to_owned()),
                );
            }
            continue;
        };
        record_change(
            events,
            current.collected_at,
            container,
            DockerInventoryEventKind::ImageChanged,
            old.image != container.image || old.image_id != container.image_id,
            Some(format_image_reference(old)),
            Some(format_image_reference(container)),
        );
        record_change(
            events,
            current.collected_at,
            container,
            DockerInventoryEventKind::StateChanged,
            old.state != container.state,
            Some(old.state.clone()),
            Some(container.state.clone()),
        );
        record_change(
            events,
            current.collected_at,
            container,
            DockerInventoryEventKind::HealthChanged,
            old.health != container.health && old.health.is_some() && container.health.is_some(),
            old.health.clone(),
            container.health.clone(),
        );
        if let (Some(old_count), Some(new_count)) = (old.restart_count, container.restart_count) {
            if new_count > old_count {
                push_event(
                    events,
                    current.collected_at,
                    container,
                    DockerInventoryEventKind::RestartCountIncreased,
                    Some(old_count.to_string()),
                    Some(new_count.to_string()),
                );
            }
        }
        if old.oom_killed != Some(true) && container.oom_killed == Some(true) {
            push_event(
                events,
                current.collected_at,
                container,
                DockerInventoryEventKind::OomKilled,
                Some("false".to_owned()),
                Some("true".to_owned()),
            );
        }
    }
}

fn record_removed_container_events(
    previous: Option<&PersistedTargetDockerInventory>,
    current_by_id: &HashMap<&str, &DockerContainerInfo>,
    observed_at: DateTime<Utc>,
    events: &mut Vec<DockerInventoryEvent>,
) {
    let Some(previous) = previous else {
        return;
    };
    for container in &previous.containers {
        if !current_by_id.contains_key(container.id.as_str()) {
            push_event(
                events,
                observed_at,
                container,
                DockerInventoryEventKind::Removed,
                Some("present".to_owned()),
                None,
            );
        }
    }
}

fn format_image_reference(container: &DockerContainerInfo) -> String {
    container.image_id.as_ref().map_or_else(
        || container.image.clone(),
        |image_id| format!("{} ({image_id})", container.image),
    )
}

fn record_change(
    events: &mut Vec<DockerInventoryEvent>,
    observed_at: DateTime<Utc>,
    container: &DockerContainerInfo,
    kind: DockerInventoryEventKind,
    changed: bool,
    previous_value: Option<String>,
    current_value: Option<String>,
) {
    if changed {
        push_event(
            events,
            observed_at,
            container,
            kind,
            previous_value,
            current_value,
        );
    }
}

fn push_event(
    events: &mut Vec<DockerInventoryEvent>,
    observed_at: DateTime<Utc>,
    container: &DockerContainerInfo,
    kind: DockerInventoryEventKind,
    previous_value: Option<String>,
    current_value: Option<String>,
) {
    events.push(DockerInventoryEvent {
        observed_at,
        container_id: container.id.clone(),
        container_name: container.name.clone(),
        kind,
        previous_value,
        current_value,
    });
}

impl TargetDockerInventoryRepository {
    pub(crate) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn save(
        &self,
        target_id: TargetId,
        inventory: &mut PersistedTargetDockerInventory,
    ) -> Result<(), RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SELECT id FROM targets WHERE id = $1 FOR UPDATE")
            .bind(target_id.as_uuid())
            .fetch_optional(&mut *transaction)
            .await?;
        let previous_row = sqlx::query(
            "SELECT collected_at, containers, events FROM target_docker_inventory WHERE target_id = $1 FOR UPDATE",
        )
        .bind(target_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await?;
        let previous = previous_row
            .map(|row| {
                let containers: serde_json::Value = row.try_get("containers")?;
                let events: serde_json::Value = row.try_get("events")?;
                Ok::<_, RepositoryError>(PersistedTargetDockerInventory {
                    collected_at: row.try_get("collected_at")?,
                    containers: serde_json::from_value(containers)
                        .map_err(RepositoryError::Serialization)?,
                    events: serde_json::from_value(events)
                        .map_err(RepositoryError::Serialization)?,
                })
            })
            .transpose()?;
        update_docker_inventory_events(previous.as_ref(), inventory);
        let containers =
            serde_json::to_value(&inventory.containers).map_err(RepositoryError::Serialization)?;
        let events =
            serde_json::to_value(&inventory.events).map_err(RepositoryError::Serialization)?;
        sqlx::query(
            "INSERT INTO target_docker_inventory (target_id, collected_at, containers, events) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (target_id) DO UPDATE SET collected_at = EXCLUDED.collected_at, containers = EXCLUDED.containers, events = EXCLUDED.events",
        )
        .bind(target_id.as_uuid())
        .bind(inventory.collected_at)
        .bind(containers)
        .bind(events)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn get(
        &self,
        target_id: TargetId,
    ) -> Result<Option<PersistedTargetDockerInventory>, RepositoryError> {
        let Some(row) = sqlx::query(
            "SELECT collected_at, containers, events FROM target_docker_inventory WHERE target_id = $1",
        )
        .bind(target_id.as_uuid())
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        let containers: serde_json::Value = row.try_get("containers")?;
        let events: serde_json::Value = row.try_get("events")?;
        Ok(Some(PersistedTargetDockerInventory {
            collected_at: row.try_get("collected_at")?,
            containers: serde_json::from_value(containers)
                .map_err(RepositoryError::Serialization)?,
            events: serde_json::from_value(events).map_err(RepositoryError::Serialization)?,
        }))
    }

    pub async fn prune_expired_events(&self) -> Result<u64, RepositoryError> {
        let cutoff = Utc::now() - chrono::Duration::days(DOCKER_INVENTORY_EVENT_RETENTION_DAYS);
        let result = sqlx::query(
            "WITH expired_inventories AS (\
               SELECT target_id FROM target_docker_inventory \
               WHERE jsonb_array_length(events) > 0 \
                 AND EXISTS (\
                   SELECT 1 FROM jsonb_array_elements(events) AS event \
                   WHERE (event->>'observed_at')::timestamptz < $1\
                 ) \
               ORDER BY collected_at LIMIT $2\
             ) UPDATE target_docker_inventory AS inventory \
             SET events = COALESCE(( \
               SELECT jsonb_agg(event ORDER BY event->>'observed_at') \
               FROM jsonb_array_elements(inventory.events) AS event \
               WHERE (event->>'observed_at')::timestamptz >= $1 \
             ), '[]'::jsonb) \
             FROM expired_inventories \
             WHERE inventory.target_id = expired_inventories.target_id",
        )
        .bind(cutoff)
        .bind(DOCKER_EVENT_PRUNE_BATCH_SIZE)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docker_container(
        id: &str,
        image: &str,
        health: Option<&str>,
        restarts: Option<u64>,
    ) -> DockerContainerInfo {
        DockerContainerInfo {
            id: id.to_owned(),
            name: format!("{id}-name"),
            image: image.to_owned(),
            state: "running".to_owned(),
            status: "Up".to_owned(),
            ports: Vec::new(),
            started_at: None,
            created_at: None,
            image_id: None,
            restart_count: restarts,
            health: health.map(str::to_owned),
            oom_killed: None,
            labels: Vec::new(),
        }
    }

    #[test]
    fn inventory_events_capture_changes_without_repeating_unchanged_samples() {
        let at = Utc::now();
        let previous = PersistedTargetDockerInventory {
            collected_at: at,
            containers: vec![docker_container("old", "nginx:1", Some("healthy"), Some(1))],
            events: Vec::new(),
        };
        let mut current = PersistedTargetDockerInventory {
            collected_at: at + chrono::Duration::seconds(30),
            containers: vec![
                docker_container("old", "nginx:2", Some("unhealthy"), Some(2)),
                docker_container("new", "redis:7", None, None),
            ],
            events: Vec::new(),
        };
        update_docker_inventory_events(Some(&previous), &mut current);
        assert_eq!(current.events.len(), 4);
        assert!(
            current
                .events
                .iter()
                .any(|event| event.kind == DockerInventoryEventKind::Added)
        );
        assert!(
            current
                .events
                .iter()
                .any(|event| event.kind == DockerInventoryEventKind::ImageChanged)
        );
        assert!(
            current
                .events
                .iter()
                .any(|event| event.kind == DockerInventoryEventKind::HealthChanged)
        );
        assert!(
            current
                .events
                .iter()
                .any(|event| event.kind == DockerInventoryEventKind::RestartCountIncreased)
        );

        let mut unchanged = PersistedTargetDockerInventory {
            collected_at: at + chrono::Duration::seconds(60),
            containers: current.containers.clone(),
            events: Vec::new(),
        };
        update_docker_inventory_events(Some(&current), &mut unchanged);
        assert_eq!(unchanged.events, current.events);
    }

    #[test]
    fn first_inventory_does_not_report_every_container_as_new() {
        let mut first = PersistedTargetDockerInventory {
            collected_at: Utc::now(),
            containers: vec![docker_container("first", "nginx:1", None, None)],
            events: Vec::new(),
        };
        update_docker_inventory_events(None, &mut first);
        assert!(first.events.is_empty());
    }

    #[test]
    fn inventory_events_include_oom_killed_transition_once() {
        let at = Utc::now();
        let mut old = docker_container("web", "nginx:1", Some("healthy"), Some(0));
        old.oom_killed = Some(false);
        let previous = PersistedTargetDockerInventory {
            collected_at: at,
            containers: vec![old],
            events: Vec::new(),
        };
        let mut killed = docker_container("web", "nginx:1", Some("unhealthy"), Some(1));
        killed.oom_killed = Some(true);
        let mut current = PersistedTargetDockerInventory {
            collected_at: at + chrono::Duration::seconds(30),
            containers: vec![killed],
            events: Vec::new(),
        };
        update_docker_inventory_events(Some(&previous), &mut current);
        assert_eq!(
            current
                .events
                .iter()
                .filter(|event| event.kind == DockerInventoryEventKind::OomKilled)
                .count(),
            1
        );
    }
}
