//! Confirmed, revalidated and idempotent update execution boundary.

use std::collections::HashMap;

use chrono::Utc;
use lxcup_core::{Execution, ExecutionId, ExecutionStatus, PlanStatus, UpdatePlan};
use lxcup_planner::{PlannerError, PlannerInput, Revalidation, UpdatePlanner};
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionRequest {
    pub plan: UpdatePlan,
    pub current_plan_input: PlannerInput,
    pub actor: String,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ExecutionEvent {
    pub sequence: u64,
    pub event_type: String,
    pub message: String,
    pub created_at: chrono::DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AuditRecord {
    pub event_type: String,
    pub actor: String,
    pub execution_id: ExecutionId,
    pub details: serde_json::Value,
    pub created_at: chrono::DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Observation returned by an agent during reconciliation after a restart or
/// connection loss.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedExecutionState {
    Running,
    Succeeded,
    Failed,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconciliationAction {
    KeepRunning,
    MarkSucceeded,
    MarkFailed,
    RequireManualReview,
}

pub trait PackageExecutor {
    fn execute(&self, plan: &UpdatePlan) -> Result<ExecutionResult, ExecutionError>;
}

/// Safe command adapter for a validated Debian/Ubuntu update plan.
#[derive(Clone, Copy, Debug, Default)]
pub struct ValidatedAptExecutor;

impl ValidatedAptExecutor {
    pub fn command(plan: &UpdatePlan) -> Result<Vec<String>, ExecutionError> {
        if plan.status != PlanStatus::Confirmed {
            return Err(ExecutionError::PlanNotConfirmed);
        }
        let packages: Vec<_> = plan
            .resolved_changes
            .iter()
            .map(|change| change.package.as_str().to_owned())
            .collect();
        if packages.is_empty() {
            return Err(ExecutionError::EmptyPlan);
        }
        if packages.iter().any(|package| {
            package.is_empty()
                || package.chars().any(char::is_whitespace)
                || package.starts_with('-')
        }) {
            return Err(ExecutionError::UnsafePackageArgument);
        }
        let mut command = vec![
            "apt-get".to_owned(),
            "--yes".to_owned(),
            "--only-upgrade".to_owned(),
            "install".to_owned(),
        ];
        command.extend(packages);
        Ok(command)
    }
}

impl PackageExecutor for ValidatedAptExecutor {
    fn execute(&self, plan: &UpdatePlan) -> Result<ExecutionResult, ExecutionError> {
        let _ = Self::command(plan)?;
        Err(ExecutionError::NotConfigured)
    }
}

#[derive(Default)]
pub struct ExecutionCoordinator {
    executions: HashMap<ExecutionId, Execution>,
    plans: HashMap<ExecutionId, UpdatePlan>,
    idempotency: HashMap<String, ExecutionId>,
    events: HashMap<ExecutionId, Vec<ExecutionEvent>>,
    audit: Vec<AuditRecord>,
}

impl ExecutionCoordinator {
    pub fn start(&mut self, request: ExecutionRequest) -> Result<ExecutionStart, ExecutionError> {
        if request.actor.trim().is_empty() || request.idempotency_key.trim().is_empty() {
            return Err(ExecutionError::MissingRequestIdentity);
        }
        if let Some(existing) = self.idempotency.get(&request.idempotency_key) {
            return Ok(ExecutionStart::Duplicate(*existing));
        }
        if request.plan.status != PlanStatus::Confirmed {
            return Err(ExecutionError::PlanNotConfirmed);
        }

        let mut plan = request.plan;
        match UpdatePlanner.revalidate(&mut plan, request.current_plan_input)? {
            Revalidation::Changed => return Err(ExecutionError::PlanChanged),
            Revalidation::Unchanged => {}
        }

        let execution = Execution::new(plan.id);
        let id = execution.id;
        let plan_id = plan.id;
        let plan_hash = plan.plan_hash.clone();
        self.idempotency.insert(request.idempotency_key, id);
        self.plans.insert(id, plan);
        self.executions.insert(id, execution.clone());
        self.append_event(id, "queued", "execution queued");
        self.audit.push(AuditRecord {
            event_type: "execution_queued".to_owned(),
            actor: request.actor,
            execution_id: id,
            details: json!({ "plan_id": plan_id.as_uuid(), "plan_hash": plan_hash }),
            created_at: Utc::now(),
        });
        Ok(ExecutionStart::Created(execution))
    }

    pub fn run<E: PackageExecutor>(
        &mut self,
        id: ExecutionId,
        executor: &E,
    ) -> Result<Execution, ExecutionError> {
        let plan = self
            .plan(id)
            .cloned()
            .ok_or(ExecutionError::UnknownExecution)?;
        self.begin(id)?;
        match executor.execute(&plan) {
            Ok(result) => self.finish(id, result),
            Err(error) => {
                let _ = self.finish(
                    id,
                    ExecutionResult {
                        exit_code: 1,
                        stdout: String::new(),
                        stderr: error.to_string(),
                    },
                );
                Err(error)
            }
        }
    }

    pub fn finish(
        &mut self,
        id: ExecutionId,
        result: ExecutionResult,
    ) -> Result<Execution, ExecutionError> {
        let next = if result.exit_code == 0 {
            ExecutionStatus::Succeeded
        } else {
            ExecutionStatus::Failed
        };
        let finished = {
            let execution = self
                .executions
                .get_mut(&id)
                .ok_or(ExecutionError::UnknownExecution)?;
            execution
                .transition_to(next, Utc::now())
                .map_err(|_| ExecutionError::InvalidState)?;
            execution.clone()
        };
        let message = if result.exit_code == 0 {
            "execution succeeded"
        } else {
            "execution failed"
        };
        self.append_event(id, &format!("{:?}", next).to_ascii_lowercase(), message);
        Ok(finished)
    }

    pub fn abort(&mut self, id: ExecutionId) -> Result<Execution, ExecutionError> {
        let aborted = {
            let execution = self
                .executions
                .get_mut(&id)
                .ok_or(ExecutionError::UnknownExecution)?;
            execution
                .transition_to(ExecutionStatus::Aborted, Utc::now())
                .map_err(|_| ExecutionError::InvalidState)?;
            execution.clone()
        };
        self.append_event(id, "aborted", "execution aborted");
        Ok(aborted)
    }

    pub fn find(&self, id: ExecutionId) -> Option<&Execution> {
        self.executions.get(&id)
    }

    pub fn begin(&mut self, id: ExecutionId) -> Result<UpdatePlan, ExecutionError> {
        let plan = self
            .plan(id)
            .cloned()
            .ok_or(ExecutionError::UnknownExecution)?;
        {
            let execution = self
                .executions
                .get_mut(&id)
                .ok_or(ExecutionError::UnknownExecution)?;
            execution
                .transition_to(ExecutionStatus::Running, Utc::now())
                .map_err(|_| ExecutionError::InvalidState)?;
        }
        self.append_event(id, "running", "execution started");
        Ok(plan)
    }

    pub fn plan(&self, id: ExecutionId) -> Option<&UpdatePlan> {
        self.plans.get(&id)
    }

    /// Moves an execution to `unknown` when its remote state cannot be read.
    pub fn mark_connection_lost(&mut self, id: ExecutionId) -> Result<Execution, ExecutionError> {
        let became_unknown = {
            let execution = self
                .executions
                .get_mut(&id)
                .ok_or(ExecutionError::UnknownExecution)?;
            if execution.status == ExecutionStatus::Running {
                execution
                    .transition_to(ExecutionStatus::Unknown, Utc::now())
                    .map_err(|_| ExecutionError::InvalidState)?;
                true
            } else {
                false
            }
        };
        if became_unknown {
            self.append_event(
                id,
                "unknown",
                "remote agent connection lost; reconciliation required",
            );
        }
        self.find(id)
            .cloned()
            .ok_or(ExecutionError::UnknownExecution)
    }

    /// Applies an observed remote state without allowing unsafe transitions.
    pub fn reconcile(
        &mut self,
        id: ExecutionId,
        observed: ObservedExecutionState,
    ) -> Result<(Execution, ReconciliationAction), ExecutionError> {
        let current = self
            .executions
            .get(&id)
            .ok_or(ExecutionError::UnknownExecution)?
            .status;
        let action = match (current, observed) {
            (
                ExecutionStatus::Succeeded | ExecutionStatus::Failed | ExecutionStatus::Aborted,
                _,
            ) => ReconciliationAction::RequireManualReview,
            (_, ObservedExecutionState::Running) => ReconciliationAction::KeepRunning,
            (_, ObservedExecutionState::Succeeded) => {
                let execution = self.finish(
                    id,
                    ExecutionResult {
                        exit_code: 0,
                        stdout: String::new(),
                        stderr: String::new(),
                    },
                )?;
                return Ok((execution, ReconciliationAction::MarkSucceeded));
            }
            (_, ObservedExecutionState::Failed) => {
                let execution = self.finish(
                    id,
                    ExecutionResult {
                        exit_code: 1,
                        stdout: String::new(),
                        stderr: "remote agent reported failure".to_owned(),
                    },
                )?;
                return Ok((execution, ReconciliationAction::MarkFailed));
            }
            (_, ObservedExecutionState::Unknown) => ReconciliationAction::RequireManualReview,
        };
        Ok((
            self.find(id)
                .cloned()
                .ok_or(ExecutionError::UnknownExecution)?,
            action,
        ))
    }

    pub fn events(&self, id: ExecutionId) -> &[ExecutionEvent] {
        self.events.get(&id).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn audit_records(&self) -> &[AuditRecord] {
        &self.audit
    }

    fn append_event(&mut self, id: ExecutionId, event_type: &str, message: &str) {
        let events = self.events.entry(id).or_default();
        events.push(ExecutionEvent {
            sequence: events.len() as u64 + 1,
            event_type: event_type.to_owned(),
            message: message.to_owned(),
            created_at: Utc::now(),
        });
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionStart {
    Created(Execution),
    Duplicate(ExecutionId),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ExecutionError {
    #[error("execution actor and idempotency key are required")]
    MissingRequestIdentity,
    #[error("plan must be explicitly confirmed")]
    PlanNotConfirmed,
    #[error("plan changed during pre-execution validation")]
    PlanChanged,
    #[error("execution has already reached a terminal state or has an invalid state")]
    InvalidState,
    #[error("execution does not exist")]
    UnknownExecution,
    #[error("execution plan must contain at least one package change")]
    EmptyPlan,
    #[error("plan contains an unsafe package argument")]
    UnsafePackageArgument,
    #[error("the package executor is not configured")]
    NotConfigured,
    #[error(transparent)]
    Planner(#[from] PlannerError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use lxcup_core::{
        ContainerId, PackageChangeKind, PackageName, PackageVersion, ResolvedPackageChange,
    };

    fn confirmed_plan() -> UpdatePlan {
        let mut plan = UpdatePlan::new(
            ContainerId::new(101),
            vec![PackageName::new("openssl").unwrap()],
            vec![ResolvedPackageChange {
                package: PackageName::new("openssl").unwrap(),
                from_version: Some(PackageVersion::new("3.0").unwrap()),
                to_version: Some(PackageVersion::new("3.1").unwrap()),
                kind: PackageChangeKind::Upgrade,
            }],
        )
        .unwrap();
        plan.transition_to(PlanStatus::Ready).unwrap();
        plan.transition_to(PlanStatus::Confirmed).unwrap();
        plan.refresh_hash();
        plan
    }

    fn request(plan: UpdatePlan, key: &str) -> ExecutionRequest {
        ExecutionRequest {
            current_plan_input: PlannerInput {
                container_id: plan.container_id,
                requested_packages: vec!["openssl".to_owned()],
                dry_run_changes: vec![lxcup_planner::DryRunChange {
                    package: "openssl".to_owned(),
                    from_version: Some("3.0".to_owned()),
                    to_version: Some("3.1".to_owned()),
                    kind: PackageChangeKind::Upgrade,
                    held: false,
                    authenticated: true,
                }],
                distribution_upgrade: false,
            },
            plan,
            actor: "operator@example.test".to_owned(),
            idempotency_key: key.to_owned(),
        }
    }

    #[test]
    fn execution_requires_confirmation_and_revalidation() {
        let mut coordinator = ExecutionCoordinator::default();
        let mut draft = confirmed_plan();
        draft.status = PlanStatus::Draft;
        assert_eq!(
            coordinator.start(request(draft, "a")),
            Err(ExecutionError::PlanNotConfirmed)
        );

        let mut changed = request(confirmed_plan(), "b");
        changed.current_plan_input.dry_run_changes[0].to_version = Some("3.2".to_owned());
        assert_eq!(coordinator.start(changed), Err(ExecutionError::PlanChanged));
    }

    #[test]
    fn idempotency_returns_the_original_execution() {
        let mut coordinator = ExecutionCoordinator::default();
        let created = coordinator
            .start(request(confirmed_plan(), "same-key"))
            .unwrap();
        let id = match created {
            ExecutionStart::Created(execution) => execution.id,
            _ => panic!(),
        };
        assert_eq!(
            coordinator.start(request(confirmed_plan(), "same-key")),
            Ok(ExecutionStart::Duplicate(id))
        );
    }

    #[test]
    fn connection_loss_requires_reconciliation_before_final_state() {
        let mut coordinator = ExecutionCoordinator::default();
        let start = coordinator
            .start(request(confirmed_plan(), "connection-loss"))
            .unwrap();
        let id = match start {
            ExecutionStart::Created(execution) => execution.id,
            _ => panic!(),
        };
        coordinator.begin(id).unwrap();
        let unknown = coordinator.mark_connection_lost(id).unwrap();
        assert_eq!(unknown.status, ExecutionStatus::Unknown);
        let (succeeded, action) = coordinator
            .reconcile(id, ObservedExecutionState::Succeeded)
            .unwrap();
        assert_eq!(succeeded.status, ExecutionStatus::Succeeded);
        assert_eq!(action, ReconciliationAction::MarkSucceeded);
    }

    #[test]
    fn command_is_fixed_and_only_uses_validated_packages() {
        let mut plan = confirmed_plan();
        assert_eq!(
            ValidatedAptExecutor::command(&plan).unwrap(),
            ["apt-get", "--yes", "--only-upgrade", "install", "openssl"]
        );
        plan.resolved_changes[0].package = PackageName::new("--option").unwrap();
        assert_eq!(
            ValidatedAptExecutor::command(&plan),
            Err(ExecutionError::UnsafePackageArgument)
        );
    }

    #[test]
    fn successful_and_failed_results_are_auditable() {
        struct SuccessfulExecutor;
        impl PackageExecutor for SuccessfulExecutor {
            fn execute(&self, _plan: &UpdatePlan) -> Result<ExecutionResult, ExecutionError> {
                Ok(ExecutionResult {
                    exit_code: 0,
                    stdout: "ok".to_owned(),
                    stderr: String::new(),
                })
            }
        }

        let mut coordinator = ExecutionCoordinator::default();
        let execution = match coordinator
            .start(request(confirmed_plan(), "audit-key"))
            .unwrap()
        {
            ExecutionStart::Created(value) => value,
            _ => unreachable!(),
        };
        let finished = coordinator.run(execution.id, &SuccessfulExecutor).unwrap();
        assert_eq!(finished.status, ExecutionStatus::Succeeded);
        assert_eq!(coordinator.events(execution.id).len(), 3);
        assert_eq!(coordinator.audit_records().len(), 1);
    }
}
