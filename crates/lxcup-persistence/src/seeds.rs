//! Reproduzierbare Daten für eine isolierte Development-Datenbank.

use chrono::{DateTime, Utc};
use lxcup_core::{
    Container, ContainerId, ContainerManagementState, ContainerStatus, Node, NodeId, NodeStatus,
    OperatingSystem,
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::repositories::RepositoryError;

/// Ein zusammengehöriger Satz stabiler Development-Daten.
#[derive(Clone, Debug)]
pub struct DevelopmentSeed {
    pub node: Node,
    pub container: Container,
}

/// Erzeugt den stabilen Seed-Satz ohne Datenbankzugriff.
pub fn development_seed() -> DevelopmentSeed {
    let node_id = NodeId::from_uuid(Uuid::from_u128(0x10000000000000000000000000000001));
    let timestamp = DateTime::<Utc>::from_timestamp(1_704_067_200, 0)
        .expect("development seed timestamp is valid");
    let node = Node {
        id: node_id,
        name: "pve-dev".to_owned(),
        address: "https://pve-dev.invalid:8006".to_owned(),
        status: NodeStatus::Unknown,
        created_at: timestamp,
    };
    let container = Container {
        id: ContainerId::new(101),
        node_id,
        name: "grafana-dev".to_owned(),
        operating_system: OperatingSystem::Debian,
        status: ContainerStatus::Stopped,
        management_state: ContainerManagementState::Managed,
        discovered_at: timestamp,
    };

    DevelopmentSeed { node, container }
}

/// Schreibt den stabilen Seed-Satz idempotent in die Development-Datenbank.
pub async fn seed_development(pool: &PgPool) -> Result<DevelopmentSeed, RepositoryError> {
    let seed = development_seed();

    sqlx::query(
        "INSERT INTO nodes (id, name, address, status, created_at) VALUES ($1, $2, $3, $4, $5) ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name, address = EXCLUDED.address, status = EXCLUDED.status, created_at = EXCLUDED.created_at",
    )
    .bind(seed.node.id.as_uuid())
    .bind(&seed.node.name)
    .bind(&seed.node.address)
    .bind("unknown")
    .bind(seed.node.created_at)
    .execute(pool)
    .await?;

    sqlx::query(
        "INSERT INTO containers (id, node_id, name, operating_system, status, management_state, discovered_at) VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT (id) DO UPDATE SET node_id = EXCLUDED.node_id, name = EXCLUDED.name, operating_system = EXCLUDED.operating_system, status = EXCLUDED.status, management_state = EXCLUDED.management_state, discovered_at = EXCLUDED.discovered_at",
    )
    .bind(seed.container.id.value() as i64)
    .bind(seed.container.node_id.as_uuid())
    .bind(&seed.container.name)
    .bind("debian")
    .bind("stopped")
    .bind("managed")
    .bind(seed.container.discovered_at)
    .execute(pool)
    .await?;

    Ok(seed)
}

#[cfg(test)]
mod tests {
    use super::development_seed;

    #[test]
    fn development_seed_is_stable() {
        let first = development_seed();
        let second = development_seed();

        assert_eq!(first.node.id, second.node.id);
        assert_eq!(first.node.name, "pve-dev");
        assert_eq!(first.container.id.value(), 101);
        assert_eq!(first.container.name, second.container.name);
    }
}
