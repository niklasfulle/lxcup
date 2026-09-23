//! Domain- und Geschäftslogik von lxcup.
//!
//! Dieses Crate bleibt unabhängig von HTTP, PostgreSQL und konkreten
//! Infrastruktur- oder Betriebssystem-Adaptern.

pub mod agent;
pub mod container;
pub mod docker;
pub mod enrollment;
pub mod environment;
pub mod error;
pub mod execution;
pub mod ids;
pub mod node;
pub mod plan;
pub mod resource;
pub mod scan;
pub mod secret;
pub mod target;
pub mod update;

pub use agent::{AgentConnectionState, AgentRegistration};
pub use container::{
    Container, ContainerAction, ContainerManagementState, ContainerStatus, OperatingSystem,
};
pub use docker::{DockerWorkload, DockerWorkloadManagementState};
pub use enrollment::{Enrollment, EnrollmentState};
pub use environment::{EnvironmentStatus, ProxmoxEnvironment};
pub use error::{
    DomainError, ErrorCode, InfrastructureError, LxcupError, LxcupResult, RetryPolicy,
};
pub use execution::{Execution, ExecutionStatus};
pub use ids::{
    AgentRegistrationId, AnsibleJobId, ContainerId, EnrollmentId, EnvironmentId, ExecutionId,
    NodeId, ScanId, SecretId, TargetId, UpdatePlanId,
};
pub use node::{Node, NodeStatus};
pub use plan::{PackageChangeKind, PlanStatus, ResolvedPackageChange, UpdatePlan};
pub use resource::{
    ActorRole, Permission, ResourceAction, ResourceLifecycle, ResourceTarget, validate_action,
};
pub use scan::{Scan, ScanStatus};
pub use secret::{SecretKind, SecretMetadata, SecretScope, SecretValue};
pub use target::{Target, TargetKind, TargetState, TargetTransport};
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
