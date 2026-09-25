use super::{ApiEvent, ApiState, SecretId, TargetId};
use lxcup_core::{Target, TargetState};

pub(crate) async fn invalidate_registered_agents(state: &ApiState, secret_id: SecretId) {
    let Some(repositories) = state.repositories.as_ref() else {
        return;
    };
    let Ok(registrations) = repositories.agent_registrations.list().await else {
        return;
    };
    for mut registration in registrations
        .into_iter()
        .filter(|item| item.secret_ref == secret_id || item.ca_secret_ref == Some(secret_id))
    {
        state
            .agents
            .write()
            .await
            .remove(&registration.container_id);
        registration.record_unreachable(
            chrono::Utc::now(),
            "agent credentials changed; re-register or redeploy the agent",
        );
        let _ = repositories.agent_registrations.save(&registration).await;
        state.publish(ApiEvent::status(
            "agent",
            registration.container_id.value().to_string(),
            "credential_invalidated",
        ));
    }
}

pub(crate) async fn reset_targets_for_secret(
    state: &ApiState,
    secret_id: SecretId,
) -> (Vec<TargetId>, Vec<Target>) {
    let mut store = state.store.write().await;
    let mut affected = Vec::new();
    let mut changed = Vec::new();
    for target in &mut store.targets {
        let affected_by_secret = target.credential_secret_ref == secret_id
            || target.agent_secret_ref == secret_id
            || target.ssh_known_hosts_secret_ref == Some(secret_id);
        if !affected_by_secret {
            continue;
        }
        affected.push(target.id);
        reset_agent_target_if_needed(target, secret_id, &mut changed);
    }
    (affected, changed)
}

pub(crate) fn reset_agent_target_if_needed(
    target: &mut Target,
    secret_id: SecretId,
    changed: &mut Vec<Target>,
) {
    if target.agent_secret_ref == secret_id && target.state != TargetState::Disabled {
        target.state = TargetState::Pending;
        target.updated_at = chrono::Utc::now();
        changed.push(target.clone());
    }
}

pub(crate) async fn persist_changed_targets(state: &ApiState, targets: Vec<Target>) {
    let Some(repositories) = state.repositories.as_ref() else {
        return;
    };
    for target in targets {
        let _ = repositories.targets.update(&target).await;
    }
}

pub(crate) async fn disable_schedules_for_targets(
    state: &ApiState,
    affected_targets: Vec<TargetId>,
) {
    let disabled = {
        let mut store = state.store.write().await;
        let mut changed = Vec::new();
        for schedule in &mut store.schedules {
            let references_affected_target = schedule
                .target_ids
                .iter()
                .any(|target_id| affected_targets.contains(target_id));
            if references_affected_target && schedule.enabled {
                schedule.enabled = false;
                schedule.last_error =
                    Some("disabled because a referenced secret was revoked or rotated".to_owned());
                changed.push(schedule.clone());
            }
        }
        changed
    };
    for schedule in disabled {
        if let Some(repositories) = state.repositories.clone() {
            let _ = repositories.schedules.update(&schedule).await;
        }
        state.publish(ApiEvent::status(
            "schedule",
            schedule.id,
            "disabled_secret_invalidated",
        ));
    }
}
