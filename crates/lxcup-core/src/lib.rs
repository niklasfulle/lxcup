//! Domain- und Geschäftslogik von lxcup.
//!
//! Dieses Crate bleibt unabhängig von HTTP, PostgreSQL und konkreten
//! Proxmox- oder Betriebssystem-Adaptern.

pub mod container;
pub mod error;
pub mod execution;
pub mod ids;
pub mod node;
pub mod plan;
pub mod scan;
pub mod update;

pub use container::{Container, ContainerManagementState, ContainerStatus, OperatingSystem};
pub use error::{DomainError, InfrastructureError, LxcupError, LxcupResult};
pub use execution::{Execution, ExecutionStatus};
pub use ids::{ContainerId, ExecutionId, NodeId, ScanId, UpdatePlanId};
pub use node::{Node, NodeStatus};
pub use plan::{PlanStatus, UpdatePlan};
pub use scan::{Scan, ScanStatus};
pub use update::{AvailableUpdate, PackageName, PackageVersion, UpdateClassification};

/// Version des Core-Crates.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn exposes_package_version() {
        assert_eq!(VERSION, "0.1.0");
    }
}
