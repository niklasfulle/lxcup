use super::event;
use lxcup_ansible::{AnsibleJob, AnsibleJobStatus, JobEventKind, JobFailureCode};
use lxcup_persistence::Repositories;

pub(crate) fn check_mode_summary(stdout: &str) -> String {
    if !stdout.contains("PLAY RECAP") {
        "Prüfung lieferte keine vollständige Ansible-Zusammenfassung; Ergebnis bitte manuell prüfen.".to_owned()
    } else if recap_has_failures(stdout) {
        "Prüfung fehlgeschlagen: Ansible meldet mindestens einen fehlgeschlagenen oder nicht erreichbaren Host. Details stehen in der Worker-Ausgabe.".to_owned()
    } else if stdout
        .lines()
        .any(|line| line.trim_start().starts_with("skipping:"))
    {
        "Prüfung abgeschlossen, aber mindestens ein Ansible-Schritt wurde übersprungen und ist nicht prüfbar. Details stehen in der Worker-Ausgabe.".to_owned()
    } else if playbook_changed(stdout) {
        "Prüfung erfolgreich: Ansible hat mögliche Änderungen erkannt, aber im Check-Modus nichts angewendet.".to_owned()
    } else {
        "Prüfung erfolgreich: Ansible hat keine Änderungen erkannt und nichts angewendet."
            .to_owned()
    }
}

pub(crate) fn plan_summary(stdout: &str) -> String {
    if !stdout.contains("PLAY RECAP") {
        return "Vorschau unvollständig: Ansible hat keine vollständige Zusammenfassung geliefert. Details stehen in der technischen Ausgabe.".to_owned();
    }
    if recap_has_failures(stdout) {
        return "Vorschau fehlgeschlagen: Ansible meldet mindestens einen fehlgeschlagenen oder nicht erreichbaren Host. Es wurde nichts angewendet; Details stehen in der technischen Ausgabe.".to_owned();
    }
    if stdout
        .lines()
        .any(|line| line.trim_start().starts_with("skipping:"))
    {
        return "Vorschau teilweise nicht prüfbar: Mindestens ein Ansible-Schritt wurde übersprungen. Es wurden keine Änderungen angewendet; Details stehen in der Diff-Ausgabe.".to_owned();
    }
    let changes = stdout
        .lines()
        .skip_while(|line| !line.contains("PLAY RECAP"))
        .skip(1)
        .filter_map(|line| {
            line.split_whitespace()
                .find_map(|field| field.strip_prefix("changed=")?.parse::<u32>().ok())
        })
        .sum::<u32>();
    if changes == 0 {
        "Dry-Run erfolgreich: Es werden keine Änderungen erwartet. Es wurde nichts angewendet."
            .to_owned()
    } else {
        format!(
            "Dry-Run erfolgreich: Ansible erwartet {changes} Änderung(en). Es wurde nichts angewendet; die sichere Diff-Vorschau steht unten."
        )
    }
}

pub(crate) fn recap_has_failures(stdout: &str) -> bool {
    stdout
        .lines()
        .skip_while(|line| !line.contains("PLAY RECAP"))
        .skip(1)
        .any(|line| {
            line.split_whitespace().any(|field| {
                ["failed=", "unreachable="]
                    .iter()
                    .any(|prefix| field.strip_prefix(prefix).is_some_and(|value| value != "0"))
            })
        })
}

pub(crate) fn recap_has_skipped_tasks(stdout: &str) -> bool {
    stdout
        .lines()
        .skip_while(|line| !line.contains("PLAY RECAP"))
        .skip(1)
        .any(|line| {
            line.split_whitespace().any(|field| {
                field
                    .strip_prefix("skipped=")
                    .and_then(|value| value.parse::<u32>().ok())
                    .is_some_and(|count| count > 0)
            })
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReconciliationDecision {
    Completed,
    ChangesRemain,
    ManualReview,
}

pub(crate) fn reconciliation_decision(stdout: &str) -> ReconciliationDecision {
    if !stdout.contains("PLAY RECAP")
        || recap_has_failures(stdout)
        || recap_has_skipped_tasks(stdout)
        || stdout
            .lines()
            .any(|line| line.trim_start().starts_with("skipping:"))
    {
        return ReconciliationDecision::ManualReview;
    }
    match recap_change_count(stdout) {
        Some(0) => ReconciliationDecision::Completed,
        Some(_) => ReconciliationDecision::ChangesRemain,
        None => ReconciliationDecision::ManualReview,
    }
}

pub(crate) fn reconciliation_summary(decision: ReconciliationDecision) -> String {
    match decision {
        ReconciliationDecision::Completed => "Ist-Zustand geprüft: Es sind keine Änderungen mehr offen. Der ursprüngliche Apply-Lauf wird als abgeschlossen markiert; Reconcile hat selbst nichts geändert.".to_owned(),
        ReconciliationDecision::ChangesRemain => "Ist-Zustand geprüft: Es bleiben Änderungen offen. Der ursprüngliche Lauf wird als fehlgeschlagen markiert. Erstelle einen neuen Plan und bestätige einen neuen Apply ausdrücklich; Reconcile hat selbst nichts geändert.".to_owned(),
        ReconciliationDecision::ManualReview => "Ist-Zustand konnte nicht vollständig bewertet werden. Der ursprüngliche Lauf wird zur manuellen Prüfung als fehlgeschlagen markiert; Reconcile hat selbst nichts geändert.".to_owned(),
    }
}

pub(crate) async fn load_reconciliation_source(
    repos: &Repositories,
    job: &AnsibleJob,
) -> Result<AnsibleJob, JobFailureCode> {
    let source_id = lxcup_ansible::reconciliation_source_job_id(&job.idempotency_key)
        .ok_or(JobFailureCode::PlaybookFailed)?;
    let source = repos
        .ansible_jobs
        .find_by_id(source_id)
        .await
        .map_err(|_| JobFailureCode::WorkerUnavailable)?
        .ok_or(JobFailureCode::PlaybookFailed)?;
    if source.status != AnsibleJobStatus::ReconcileRequired
        || source.mode != lxcup_ansible::ExecutionMode::Apply
        || !source.operation.can_reconcile_apply()
        || source.target != job.target
        || source.operation != job.operation
        || source.parameters != job.parameters
        || source.secret_refs != job.secret_refs
    {
        return Err(JobFailureCode::PlaybookFailed);
    }
    Ok(source)
}

pub(crate) async fn finalize_reconciliation_source(
    repos: &Repositories,
    mut source: AnsibleJob,
    decision: ReconciliationDecision,
    message: &str,
) -> Result<(), JobFailureCode> {
    let status = if decision == ReconciliationDecision::Completed {
        AnsibleJobStatus::Succeeded
    } else {
        AnsibleJobStatus::Failed
    };
    source
        .transition_to(status)
        .map_err(|_| JobFailureCode::PlaybookFailed)?;
    repos
        .ansible_jobs
        .update(&source)
        .await
        .map_err(|_| JobFailureCode::WorkerUnavailable)?;
    event(
        repos,
        source.id,
        JobEventKind::WorkerLog {
            source: "reconcile".to_owned(),
            message: message.to_owned(),
        },
    )
    .await
    .map_err(|_| JobFailureCode::WorkerUnavailable)?;
    event(repos, source.id, JobEventKind::StatusChanged { status })
        .await
        .map_err(|_| JobFailureCode::WorkerUnavailable)?;
    Ok(())
}

pub(crate) fn classify_playbook_failure(output: &str) -> JobFailureCode {
    if output.contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
        || output.contains("Host key verification failed")
        || output.contains("host key for")
    {
        JobFailureCode::HostKeyChanged
    } else if output.contains("Invalid/incorrect password")
        || output.contains("Permission denied, please try again")
    {
        JobFailureCode::InvalidCredentials
    } else if output.contains("UNREACHABLE!") || output.contains("unreachable: true") {
        JobFailureCode::Unreachable
    } else {
        JobFailureCode::PlaybookFailed
    }
}

pub(crate) fn recap_change_count(stdout: &str) -> Option<u32> {
    let mut found = false;
    let changes = stdout
        .lines()
        .skip_while(|line| !line.contains("PLAY RECAP"))
        .flat_map(str::split_whitespace)
        .filter_map(|field| {
            let value = field.strip_prefix("changed=")?.parse::<u32>().ok()?;
            found = true;
            Some(value)
        })
        .sum();
    found.then_some(changes)
}

pub(crate) fn playbook_changed(stdout: &str) -> bool {
    recap_change_count(stdout)
        .map(|changes| changes > 0)
        .unwrap_or_else(|| {
            stdout
                .lines()
                .any(|line| line.trim_start().starts_with("changed: ["))
        })
}

pub(crate) fn finished_task_results(stdout: &str) -> Vec<(String, bool)> {
    let mut results = Vec::new();
    let mut current: Option<(String, bool, bool)> = None;
    for line in stdout.lines() {
        if let Some(task) = task_header_name(line) {
            push_finished_task(&mut current, &mut results);
            current = Some((task.to_owned(), false, false));
            continue;
        }
        update_task_result(&mut current, line);
    }
    push_finished_task(&mut current, &mut results);
    results
}

pub(crate) fn push_finished_task(
    current: &mut Option<(String, bool, bool)>,
    results: &mut Vec<(String, bool)>,
) {
    if let Some((name, changed, finished)) = current {
        if *finished {
            results.push((std::mem::take(name), *changed));
        }
    }
    *current = None;
}

pub(crate) fn update_task_result(current: &mut Option<(String, bool, bool)>, line: &str) {
    let Some((_, changed, finished)) = current.as_mut() else {
        return;
    };
    let outcome = line.trim_start();
    if outcome.starts_with("changed: [") {
        *changed = true;
        *finished = true;
    } else if is_finished_outcome(outcome) {
        *finished = true;
    }
}

pub(crate) fn is_finished_outcome(outcome: &str) -> bool {
    ["ok: [", "fatal: [", "unreachable: ["]
        .iter()
        .any(|prefix| outcome.starts_with(prefix))
}

pub(crate) fn task_header_name(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let header = line
        .strip_prefix("TASK [")
        .or_else(|| line.strip_prefix("RUNNING HANDLER ["))?;
    header.split_once(']').map(|(name, _)| name)
}

pub(crate) fn apply_summary(stdout: &str, process_succeeded: bool) -> String {
    let changed_tasks = finished_task_results(stdout)
        .iter()
        .filter(|(_, changed)| *changed)
        .count();
    if !process_succeeded {
        format!(
            "Apply fehlgeschlagen. Bis zum Fehler haben {changed_tasks} Task(s) Änderungen gemeldet; Details und Status je Task stehen im Protokoll."
        )
    } else if changed_tasks > 0 {
        format!(
            "Apply erfolgreich ausgeführt: {changed_tasks} Task(s) haben Änderungen vorgenommen. Die tatsächlichen Schritte stehen im Protokoll."
        )
    } else if playbook_changed(stdout) {
        format!(
            "Apply erfolgreich ausgeführt: Ansible meldet {} Änderung(en); einzelne Task-Ausgaben stehen im technischen Protokoll.",
            recap_change_count(stdout).unwrap_or_default()
        )
    } else {
        "Apply erfolgreich ausgeführt: Ansible meldet keine Änderungen am Zielsystem.".to_owned()
    }
}
