use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

/// Minimal validated telemetry fields needed to evaluate resource alerts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TelemetryLoadSample {
    pub collected_at: DateTime<Utc>,
    pub cpu_basis_points: Option<u16>,
    pub memory_basis_points: Option<u16>,
    pub storage_basis_points: Option<u16>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryMetric {
    Cpu,
    Memory,
    Storage,
}

impl TelemetryMetric {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Memory => "RAM",
            Self::Storage => "Speicher",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryAlertSeverity {
    Warning,
    Critical,
}

impl TelemetryAlertSeverity {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Warning => "Warnung",
            Self::Critical => "Kritisch",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TelemetryThresholdAlert {
    pub metric: TelemetryMetric,
    pub severity: TelemetryAlertSeverity,
    pub value_basis_points: u16,
    pub threshold_basis_points: u16,
    pub triggered_at: DateTime<Utc>,
    pub observed_at: DateTime<Utc>,
}

#[derive(Clone, Copy)]
struct Threshold {
    value: u16,
    duration: Duration,
}

#[derive(Clone, Copy)]
struct Rule {
    metric: TelemetryMetric,
    warning: Threshold,
    critical: Threshold,
}

const RULES: [Rule; 3] = [
    Rule {
        metric: TelemetryMetric::Cpu,
        warning: Threshold {
            value: 9_000,
            duration: Duration::minutes(5),
        },
        critical: Threshold {
            value: 9_800,
            duration: Duration::minutes(2),
        },
    },
    Rule {
        metric: TelemetryMetric::Memory,
        warning: Threshold {
            value: 9_000,
            duration: Duration::minutes(5),
        },
        critical: Threshold {
            value: 9_500,
            duration: Duration::minutes(2),
        },
    },
    Rule {
        metric: TelemetryMetric::Storage,
        warning: Threshold {
            value: 8_500,
            duration: Duration::minutes(10),
        },
        critical: Threshold {
            value: 9_500,
            duration: Duration::minutes(5),
        },
    },
];

const MAX_SAMPLE_GAP: Duration = Duration::seconds(7);

/// Fires only when recent, contiguous samples remain above a metric threshold.
/// Missing samples break the streak rather than counting as sustained load.
pub fn evaluate_telemetry_thresholds(
    samples: &[TelemetryLoadSample],
) -> Vec<TelemetryThresholdAlert> {
    let mut ordered = samples.to_vec();
    ordered.sort_by_key(|sample| sample.collected_at);
    let Some(latest) = ordered.last().copied() else {
        return Vec::new();
    };

    RULES
        .iter()
        .filter_map(|rule| evaluate_rule(*rule, &ordered, latest))
        .collect()
}

fn evaluate_rule(
    rule: Rule,
    samples: &[TelemetryLoadSample],
    latest: TelemetryLoadSample,
) -> Option<TelemetryThresholdAlert> {
    let latest_value = metric_value(latest, rule.metric)?;
    if let Some(critical_start) = sustained_start(samples, rule.metric, rule.critical.value) {
        if latest.collected_at - critical_start >= rule.critical.duration {
            return Some(TelemetryThresholdAlert {
                metric: rule.metric,
                severity: TelemetryAlertSeverity::Critical,
                value_basis_points: latest_value,
                threshold_basis_points: rule.critical.value,
                triggered_at: critical_start + rule.critical.duration,
                observed_at: latest.collected_at,
            });
        }
    }

    let warning_start = sustained_start(samples, rule.metric, rule.warning.value)?;
    (latest.collected_at - warning_start >= rule.warning.duration).then_some(
        TelemetryThresholdAlert {
            metric: rule.metric,
            severity: TelemetryAlertSeverity::Warning,
            value_basis_points: latest_value,
            threshold_basis_points: rule.warning.value,
            triggered_at: warning_start + rule.warning.duration,
            observed_at: latest.collected_at,
        },
    )
}

fn sustained_start(
    samples: &[TelemetryLoadSample],
    metric: TelemetryMetric,
    threshold: u16,
) -> Option<DateTime<Utc>> {
    let mut streak_start = None;
    let mut previous = None;
    for sample in samples.iter().rev() {
        let Some(value) = metric_value(*sample, metric) else {
            break;
        };
        if value < threshold {
            break;
        }
        if previous.is_some_and(|time| time - sample.collected_at > MAX_SAMPLE_GAP) {
            break;
        }
        streak_start = Some(sample.collected_at);
        previous = Some(sample.collected_at);
    }
    streak_start
}

fn metric_value(sample: TelemetryLoadSample, metric: TelemetryMetric) -> Option<u16> {
    match metric {
        TelemetryMetric::Cpu => sample.cpu_basis_points,
        TelemetryMetric::Memory => sample.memory_basis_points,
        TelemetryMetric::Storage => sample.storage_basis_points,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(seconds_ago: i64, cpu: u16, memory: u16, storage: u16) -> TelemetryLoadSample {
        TelemetryLoadSample {
            collected_at: Utc::now() - Duration::seconds(seconds_ago),
            cpu_basis_points: Some(cpu),
            memory_basis_points: Some(memory),
            storage_basis_points: Some(storage),
        }
    }

    #[test]
    fn sustained_cpu_warns_after_five_minutes_and_escalates_when_critical() {
        let samples: Vec<_> = (0..=61)
            .map(|index| sample(index * 5, 9_200, 3_000, 4_000))
            .collect();
        let alerts = evaluate_telemetry_thresholds(&samples);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].metric, TelemetryMetric::Cpu);
        assert_eq!(alerts[0].severity, TelemetryAlertSeverity::Warning);

        let samples: Vec<_> = (0..=61)
            .map(|index| sample(index * 5, 9_900, 3_000, 4_000))
            .collect();
        let alerts = evaluate_telemetry_thresholds(&samples);
        assert_eq!(alerts[0].severity, TelemetryAlertSeverity::Critical);
    }

    #[test]
    fn memory_and_storage_use_their_own_thresholds_and_durations() {
        let memory: Vec<_> = (0..=61)
            .map(|index| sample(index * 5, 2_000, 9_100, 4_000))
            .collect();
        let alerts = evaluate_telemetry_thresholds(&memory);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].metric, TelemetryMetric::Memory);

        let storage: Vec<_> = (0..=121)
            .map(|index| sample(index * 5, 2_000, 4_000, 8_700))
            .collect();
        let alerts = evaluate_telemetry_thresholds(&storage);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].metric, TelemetryMetric::Storage);
    }

    #[test]
    fn spikes_missing_values_and_sample_gaps_do_not_trigger_alerts() {
        let spike: Vec<_> = (0..=3)
            .map(|index| sample(index * 5, 9_900, 2_000, 3_000))
            .collect();
        assert!(evaluate_telemetry_thresholds(&spike).is_empty());

        let mut gap: Vec<_> = (0..=60)
            .map(|index| sample(index * 5, 9_200, 2_000, 3_000))
            .collect();
        gap[30].collected_at -= Duration::seconds(40);
        assert!(evaluate_telemetry_thresholds(&gap).is_empty());

        let mut missing: Vec<_> = (0..=60)
            .map(|index| sample(index * 5, 9_200, 2_000, 3_000))
            .collect();
        missing[60].cpu_basis_points = None;
        assert!(evaluate_telemetry_thresholds(&missing).is_empty());
    }

    #[test]
    fn empty_telemetry_never_triggers_load_alerts() {
        assert!(evaluate_telemetry_thresholds(&[]).is_empty());
    }
}
