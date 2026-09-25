use super::{
    ApiEnvelope, ApiError, ApiEvent, ApiState, CreateEnrollmentRequest, Enrollment, EnrollmentDto,
    envelope, parse_uuid, require_permission,
};
use axum::{
    Json,
    extract::{Extension, Json as JsonBody, Path, State},
    http::StatusCode,
};
use lxcup_ansible::{
    AnsibleJob, AnsibleJobRequest, AnsibleJobStatus, AnsibleOperation, AnsibleParameters,
    ExecutionMode, JobEventKind, JobFailureCode, JobSubmission, PlaybookRegistry,
};
use lxcup_core::{
    ActorRole, ContainerId, ContainerManagementState, EnrollmentId, EnrollmentState,
    ResourceLifecycle, ResourceTarget, SecretId, Target, TargetId, TargetState, UpdateRisk,
};
use uuid::Uuid;

mod enrollment;
mod jobs;
mod package_policy;
mod reconciliation;

pub(super) use enrollment::{create_enrollment, get_enrollment};
pub(super) use jobs::{
    create_ansible_job, get_ansible_job, get_ansible_job_events, list_ansible_jobs,
    persist_created_job, reconcile_ansible_job, retry_ansible_job,
};
pub(super) use reconciliation::{queue_agent_reconfiguration, reconcile_onboarding_jobs};

use jobs::{
    CreateAnsibleJobRequest, configured_ansible_secret_refs, map_ansible_error,
    queue_enrollment_job,
};
use package_policy::{
    find_existing_job, resolve_ansible_target, resolve_job_secret_refs, validate_package_update,
};
use reconciliation::ensure_deployment_followups;
