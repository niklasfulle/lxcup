use chrono::{Duration, Utc};

use super::availability_dto;

#[test]
fn availability_payload_distinguishes_missing_fresh_and_stale_heartbeats() {
    let now = Utc::now();
    let unavailable = availability_dto(None, now);
    assert!(!unavailable.available);
    assert!(unavailable.last_seen_at.is_none());
    assert!(unavailable.artifact_store_available.is_none());
    assert!(unavailable.artifact_store_checked_at.is_none());

    let checked_at = now - Duration::seconds(2);
    let fresh = availability_dto(
        Some(lxcup_persistence::WorkerHeartbeatStatus {
            last_seen_at: now - Duration::seconds(5),
            artifact_store_available: Some(false),
            artifact_store_checked_at: Some(checked_at),
        }),
        now,
    );
    assert!(fresh.available);
    assert_eq!(fresh.artifact_store_available, Some(false));
    assert_eq!(fresh.artifact_store_checked_at, Some(checked_at));

    let stale = availability_dto(
        Some(lxcup_persistence::WorkerHeartbeatStatus {
            last_seen_at: now - Duration::seconds(11),
            artifact_store_available: Some(true),
            artifact_store_checked_at: Some(checked_at),
        }),
        now,
    );
    assert!(!stale.available);
    assert_eq!(stale.artifact_store_available, Some(true));
}
