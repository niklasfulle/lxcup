use super::{ApiError, Target, TargetId, TargetKind};
use chrono::{Duration, Timelike, Utc};
use lxcup_agent::{AgentHeartbeat, AgentPlatform};
use lxcup_core::{InstalledPackage, PackageInventorySnapshot, PackageName, PackageVersion};

pub(super) fn sanitize_reported_package_inventory(
    heartbeat: &AgentHeartbeat,
    target_kind: TargetKind,
    target_id: TargetId,
) -> Option<PackageInventorySnapshot> {
    let reported_package_count = heartbeat
        .package_inventory
        .as_ref()
        .map(|inventory| inventory.packages.len());
    let package_inventory = match sanitize_agent_package_inventory(heartbeat, target_kind) {
        Ok(inventory) => inventory,
        Err(error) => {
            tracing::warn!(
                target: "lxcup_server::agent",
                target_id = %target_id.as_uuid(),
                error_code = %error.code,
                "discarding invalid optional package inventory while accepting agent heartbeat"
            );
            None
        }
    };
    warn_on_discarded_inventory_entries(
        target_id,
        reported_package_count,
        package_inventory.as_ref(),
    );
    package_inventory
}

fn warn_on_discarded_inventory_entries(
    target_id: TargetId,
    reported_count: Option<usize>,
    snapshot: Option<&PackageInventorySnapshot>,
) {
    let (Some(reported), Some(snapshot)) = (reported_count, snapshot) else {
        return;
    };
    if snapshot.packages.len() >= reported {
        return;
    }
    tracing::warn!(
        target: "lxcup_server::agent",
        target_id = %target_id.as_uuid(),
        reported_packages = reported,
        accepted_packages = snapshot.packages.len(),
        "discarded invalid Windows package inventory entries"
    );
}

pub(super) fn warn_on_incomplete_telemetry(
    target_id: TargetId,
    summary: &TelemetrySanitizationSummary,
) {
    if summary.rejected() == 0 && summary.missing_samples == 0 {
        return;
    }
    tracing::warn!(
        target: "lxcup_server::telemetry",
        target_id = %target_id.as_uuid(),
        received_samples = summary.received,
        rejected_samples = summary.rejected(),
        stale_samples = summary.stale,
        future_samples = summary.future,
        out_of_range_samples = summary.out_of_range,
        missing_samples = summary.missing_samples,
        duplicate_samples = summary.duplicates,
        truncated_samples = summary.truncated,
        "agent telemetry window was incomplete or contained rejected samples"
    );
}

pub(super) async fn persist_agent_heartbeat(
    repositories: &lxcup_persistence::Repositories,
    target: &Target,
    heartbeat: Option<&AgentHeartbeat>,
    package_inventory: Option<&PackageInventorySnapshot>,
    telemetry_summary: TelemetrySanitizationSummary,
) -> Result<(), ApiError> {
    repositories
        .targets
        .update(target)
        .await
        .map_err(|_| ApiError::storage())?;
    if let Some(heartbeat) = heartbeat {
        if repositories
            .telemetry
            .append_heartbeat(heartbeat)
            .await
            .is_err()
        {
            tracing::error!(
                target: "lxcup_server::telemetry",
                target_id = %target.id.as_uuid(),
                accepted_samples = telemetry_summary.accepted,
                "agent telemetry persistence failed"
            );
            return Err(ApiError::storage());
        }
    }
    if let Some(snapshot) = package_inventory {
        let latest = repositories
            .package_inventory
            .find_latest(target.id)
            .await
            .map_err(|_| ApiError::storage())?;
        if latest.is_none_or(|current| current.snapshot.collected_at < snapshot.collected_at) {
            repositories
                .package_inventory
                .replace(snapshot)
                .await
                .map_err(|_| ApiError::storage())?;
        }
    }
    Ok(())
}

fn sanitize_agent_package_inventory(
    heartbeat: &AgentHeartbeat,
    target_kind: TargetKind,
) -> Result<Option<PackageInventorySnapshot>, ApiError> {
    let Some(inventory) = heartbeat.package_inventory.as_ref() else {
        return Ok(None);
    };
    if target_kind != TargetKind::WindowsServer || heartbeat.info.platform != AgentPlatform::Windows
    {
        return Err(ApiError::bad_request(
            "agent_inventory_platform_mismatch",
            "package inventory does not match the registered Windows target",
        ));
    }
    let now = Utc::now();
    if inventory.packages.len() > 50_000
        || inventory.collected_at > now + Duration::seconds(5)
        || inventory.collected_at < now - Duration::hours(1)
    {
        return Err(ApiError::bad_request(
            "agent_inventory_invalid",
            "Windows package inventory is invalid or too old",
        ));
    }
    let mut names = std::collections::HashSet::new();
    let mut packages = Vec::with_capacity(inventory.packages.len());
    for package in &inventory.packages {
        if let Some(package) = sanitize_agent_package(package, &mut names) {
            packages.push(package);
        }
    }
    if packages.is_empty() && !inventory.packages.is_empty() {
        return Ok(None);
    }
    Ok(Some(PackageInventorySnapshot {
        target_id: TargetId::from_uuid(heartbeat.target_id),
        collected_at: inventory.collected_at,
        packages,
    }))
}

fn sanitize_agent_package(
    package: &lxcup_agent::AgentInstalledPackage,
    names: &mut std::collections::HashSet<String>,
) -> Option<InstalledPackage> {
    let source = package.source.as_deref();
    let safe_identifier = match source {
        Some("winget") => lxcup_agent::safe_winget_id(&package.name),
        Some("msstore") => lxcup_agent::safe_msstore_id(&package.name),
        Some(source @ ("arp" | "msix")) => {
            lxcup_agent::safe_read_only_windows_package_id(&package.name, source)
        }
        _ => false,
    };
    let invalid_package = !safe_identifier
        || package.name.len() > 128
        || package.installed_version.trim().is_empty()
        || package.installed_version.len() > 128
        || package
            .candidate_version
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 128)
        || !names.insert(package.name.to_ascii_lowercase());
    if invalid_package {
        return None;
    }
    let name = PackageName::new(package.name.clone()).ok()?;
    let version = PackageVersion::new(package.installed_version.clone()).ok()?;
    let candidate_version = if matches!(source, Some("winget" | "msstore")) {
        package
            .candidate_version
            .as_ref()
            .map(|value| PackageVersion::new(value.clone()))
            .transpose()
            .ok()?
    } else {
        None
    };
    Some(InstalledPackage {
        name,
        version,
        candidate_version,
        architecture: package.architecture.clone(),
        source: package.source.clone(),
    })
}

pub(super) fn sanitize_docker_telemetry_window(heartbeat: &mut AgentHeartbeat) {
    const MAX_DOCKER_SAMPLES: usize = 8_000;
    let now = Utc::now();
    let mut seen = std::collections::HashSet::new();
    heartbeat.docker_telemetry.samples.retain(|sample| {
        let valid_id = (12..=64).contains(&sample.container_id.len())
            && sample
                .container_id
                .chars()
                .all(|character| character.is_ascii_hexdigit());
        valid_id
            && sample
                .memory_basis_points
                .is_none_or(|value| value <= 10_000)
            && sample.collected_at <= now + Duration::seconds(5)
            && sample.collected_at
                >= now - Duration::seconds(lxcup_agent::TelemetryBuffer::WINDOW_SECONDS)
            && seen.insert((
                sample.container_id.to_ascii_lowercase(),
                sample.collected_at,
            ))
    });
    heartbeat
        .docker_telemetry
        .samples
        .sort_by_key(|sample| sample.collected_at);
    if heartbeat.docker_telemetry.samples.len() > MAX_DOCKER_SAMPLES {
        let excess = heartbeat.docker_telemetry.samples.len() - MAX_DOCKER_SAMPLES;
        heartbeat.docker_telemetry.samples.drain(..excess);
    }
}

/// Treat telemetry as best-effort heartbeat data: discard malformed, stale,
/// duplicate, or oversized sample windows while preserving the heartbeat.
/// This bounds persistence work even for an authenticated but buggy agent.
#[derive(Debug, Default, Eq, PartialEq)]
pub(super) struct TelemetrySanitizationSummary {
    pub(super) received: usize,
    pub(super) accepted: usize,
    pub(super) stale: usize,
    pub(super) future: usize,
    pub(super) out_of_range: usize,
    pub(super) missing_samples: usize,
    pub(super) duplicates: usize,
    pub(super) truncated: usize,
}

impl TelemetrySanitizationSummary {
    pub(super) fn rejected(&self) -> usize {
        self.stale + self.future + self.out_of_range + self.duplicates + self.truncated
    }
}

pub(super) fn sanitize_telemetry_window(
    heartbeat: &mut AgentHeartbeat,
) -> TelemetrySanitizationSummary {
    const MAX_SAMPLES: usize = 32;
    let samples = &mut heartbeat.telemetry.samples;
    let mut summary = TelemetrySanitizationSummary {
        received: samples.len(),
        ..TelemetrySanitizationSummary::default()
    };
    if samples.len() > MAX_SAMPLES {
        summary.truncated = samples.len() - MAX_SAMPLES;
        samples.drain(..samples.len() - MAX_SAMPLES);
        heartbeat.telemetry.partial = true;
    }

    let received_at = chrono::Utc::now();
    let max_sample_age = chrono::Duration::seconds(lxcup_agent::TelemetryBuffer::WINDOW_SECONDS);
    let max_future_skew = chrono::Duration::seconds(-5);
    let sent_at = heartbeat.sent_at;
    samples.retain_mut(|sample| {
        let sample_age = sent_at.signed_duration_since(sample.collected_at);
        if sample_age > max_sample_age {
            summary.stale += 1;
            return false;
        }
        if sample_age < max_future_skew {
            summary.future += 1;
            return false;
        }
        if sample.cpu_basis_points.is_some_and(|value| value > 10_000)
            || sample
                .memory_basis_points
                .is_some_and(|value| value > 10_000)
            || sample
                .storage_basis_points
                .is_some_and(|value| value > 10_000)
        {
            summary.out_of_range += 1;
            return false;
        }
        let normalized_at = received_at - sample_age;
        // Strip transport-jitter fractions so overlapping heartbeats hit the
        // same target/timestamp database key and are deduplicated on insert.
        sample.collected_at = normalized_at.with_nanosecond(0).unwrap_or(normalized_at);
        true
    });
    if summary.stale + summary.future + summary.out_of_range > 0 {
        heartbeat.telemetry.partial = true;
    }

    samples.sort_by_key(|sample| sample.collected_at);
    samples.dedup_by(|right, left| {
        let duplicate = right.collected_at == left.collected_at;
        if duplicate {
            summary.duplicates += 1;
        }
        duplicate
    });
    if summary.duplicates > 0 {
        heartbeat.telemetry.partial = true;
    }
    summary.missing_samples = samples
        .windows(2)
        .map(|pair| (pair[1].collected_at - pair[0].collected_at).num_milliseconds())
        .filter(|gap_ms| *gap_ms > 7_500)
        .map(|gap_ms| ((gap_ms + 4_999) / 5_000 - 1) as usize)
        .sum();
    if summary.missing_samples > 0 {
        heartbeat.telemetry.partial = true;
    }
    summary.accepted = samples.len();
    summary
}
