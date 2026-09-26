//! Wiederverwendbare, versionierte Domain-Fixtures für Tests.

use chrono::Utc;
use lxcup_core::{
    AvailableUpdate, Container, ContainerId, ContainerStatus, Execution, ExecutionStatus,
    OperatingSystem, PackageChangeKind, PackageName, PackageVersion, PlanStatus,
    ResolvedPackageChange, Scan, ScanStatus, UpdateClassification, UpdatePlan,
};

/// Deterministic output used by APT and agent adapter tests.
pub fn apt_machine_output() -> &'static str {
    "Package: openssl\nInstalled: 3.5.0-1\nCandidate: 3.5.1-1\nSecurity: yes\nHeld: no\nAuthenticated: yes"
}

/// Deterministic Windows-agent metrics fixture for future Windows coverage.
pub fn windows_agent_metrics_output() -> &'static str {
    r#"{"agent":"windows","version":"0.3.1","reboot_required":false,"updates":0}"#
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MockAgentResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn successful_agent_result() -> MockAgentResult {
    MockAgentResult {
        exit_code: 0,
        stdout: "ok".to_owned(),
        stderr: String::new(),
    }
}

/// Erzeugt einen typischen verwaltbaren Debian-Container.
pub fn container() -> Container {
    Container::new(
        ContainerId::new(101),
        lxcup_core::NodeId::new(),
        "grafana",
        OperatingSystem::Debian,
        ContainerStatus::Running,
    )
    .expect("fixture container is valid")
}

/// Erzeugt einen erfolgreichen Scan mit Security- und Normal-Update.
pub fn succeeded_scan() -> Scan {
    let mut scan = Scan::new(ContainerId::new(101));
    scan.updates = vec![
        AvailableUpdate {
            package: package("openssl"),
            installed_version: version("3.5.0-1"),
            candidate_version: version("3.5.1-1"),
            classification: UpdateClassification::Security,
            held: false,
        },
        AvailableUpdate {
            package: package("curl"),
            installed_version: version("8.14.0-1"),
            candidate_version: version("8.14.1-1"),
            classification: UpdateClassification::Normal,
            held: false,
        },
    ];
    let now = Utc::now();
    scan.transition_to(ScanStatus::Running, now)
        .expect("fixture scan can start");
    scan.transition_to(ScanStatus::Succeeded, now)
        .expect("fixture scan can succeed");
    scan
}

/// Erzeugt einen aufgelösten und bestätigungsfähigen Update-Plan.
pub fn ready_plan() -> UpdatePlan {
    let mut plan = plan_with_status();
    plan.transition_to(PlanStatus::Ready)
        .expect("fixture plan can become ready");
    plan
}

/// Erzeugt einen wegen einer Safety-Regel blockierten Update-Plan.
pub fn blocked_plan() -> UpdatePlan {
    let mut plan = plan_with_status();
    plan.transition_to(PlanStatus::Blocked)
        .expect("fixture plan can become blocked");
    plan
}

/// Erzeugt eine erfolgreich abgeschlossene Update-Ausführung.
pub fn successful_execution() -> Execution {
    let mut execution = Execution::new(lxcup_core::UpdatePlanId::new());
    let now = Utc::now();
    execution
        .transition_to(ExecutionStatus::Running, now)
        .expect("fixture execution can start");
    execution
        .transition_to(ExecutionStatus::Succeeded, now)
        .expect("fixture execution can succeed");
    execution
}

fn plan_with_status() -> UpdatePlan {
    UpdatePlan::new(
        ContainerId::new(101),
        vec![package("openssl")],
        vec![ResolvedPackageChange {
            package: package("openssl"),
            from_version: Some(version("3.5.0-1")),
            to_version: Some(version("3.5.1-1")),
            kind: PackageChangeKind::Upgrade,
        }],
    )
    .expect("fixture plan is valid")
}

fn package(value: &str) -> PackageName {
    PackageName::new(value).expect("fixture package is valid")
}

fn version(value: &str) -> PackageVersion {
    PackageVersion::new(value).expect("fixture version is valid")
}

#[cfg(test)]
mod tests {
    use super::{
        apt_machine_output, blocked_plan, container, ready_plan, succeeded_scan,
        successful_agent_result, successful_execution, windows_agent_metrics_output,
    };
    use lxcup_core::{
        ContainerStatus, ExecutionStatus, PlanStatus, ScanStatus, UpdateClassification,
    };

    #[test]
    fn fixtures_cover_successful_mvp_scenario() {
        assert_eq!(container().status, ContainerStatus::Running);
        assert_eq!(succeeded_scan().status, ScanStatus::Succeeded);
        assert_eq!(ready_plan().status, PlanStatus::Ready);
        assert_eq!(successful_execution().status, ExecutionStatus::Succeeded);
    }

    #[test]
    fn fixtures_cover_security_and_blocked_scenarios() {
        let scan = succeeded_scan();

        assert!(
            scan.updates
                .iter()
                .any(|update| update.classification == UpdateClassification::Security)
        );
        assert_eq!(blocked_plan().status, PlanStatus::Blocked);
    }

    #[test]
    fn adapter_fixtures_are_stable_and_safe_to_log() {
        assert!(apt_machine_output().contains("openssl"));
        assert!(windows_agent_metrics_output().contains("windows"));
        assert_eq!(successful_agent_result().exit_code, 0);
    }
}
