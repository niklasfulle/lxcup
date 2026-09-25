use super::*;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use lxcup_ansible::{
    AnsibleJobRequest, AnsibleJobStatus, AnsibleOperation, AnsibleParameters, ExecutionMode,
    JobSubmission,
};
use lxcup_core::{JobSchedule, Node, NodeId, ScheduleFrequency};
use lxcup_secrets::CreateSecret;
use tower::ServiceExt;

mod agents;
mod core_state;
mod docker_enrollment;
mod inventory;
mod reconciliation;
mod removed_resources;
mod workflows;
