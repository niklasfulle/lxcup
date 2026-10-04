use std::collections::HashMap;

use super::{AgentInstalledPackage, DockerContainerInfo, DockerTelemetrySample};
use chrono::{DateTime, Utc};

pub fn parse_docker_stats(output: &str, collected_at: DateTime<Utc>) -> Vec<DockerTelemetrySample> {
    output
        .lines()
        .filter_map(|line| {
            let fields = line.split('\t').collect::<Vec<_>>();
            if fields.len() != 4 || !fields[0].trim().chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            let percentage = |value: &str, maximum: f64| {
                value
                    .trim()
                    .trim_end_matches('%')
                    .parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite() && (0.0..=maximum).contains(value))
                    .map(|value| (value * 100.0).round() as u16)
            };
            let (used, limit) = fields[2].split_once('/')?;
            let used = parse_docker_size(used.trim());
            let limit = parse_docker_size(limit.trim());
            Some(DockerTelemetrySample {
                collected_at,
                container_id: fields[0].trim().to_ascii_lowercase(),
                cpu_basis_points: percentage(fields[1], 655.35),
                memory_basis_points: percentage(fields[3], 100.0),
                memory_used_bytes: used,
                memory_limit_bytes: limit,
            })
        })
        .collect()
}

pub fn parse_remote_image_config_digest(output: &str, platform: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(output).ok()?;
    let entries = value.as_array().cloned().unwrap_or_else(|| vec![value]);
    for entry in &entries {
        let descriptor_platform = entry
            .pointer("/Descriptor/platform")
            .or_else(|| entry.pointer("/descriptor/platform"))
            .or_else(|| entry.get("Platform"))
            .or_else(|| entry.get("platform"));
        let platform_matches = descriptor_platform.is_none_or(|value| {
            let os = value.get("os").and_then(serde_json::Value::as_str);
            let architecture = value
                .get("architecture")
                .and_then(serde_json::Value::as_str);
            format!(
                "{}/{}",
                os.unwrap_or_default(),
                architecture.unwrap_or_default()
            ) == platform
        });
        if !platform_matches {
            continue;
        }
        let manifest = entry
            .get("SchemaV2Manifest")
            .or_else(|| entry.get("schema2Manifest"))
            .or_else(|| entry.get("manifest"))
            .unwrap_or(entry);
        let digest = manifest
            .pointer("/config/digest")
            .or_else(|| manifest.pointer("/Config/Digest"))
            .and_then(serde_json::Value::as_str)?;
        if safe_image_id(digest).is_some() {
            return Some(digest.to_owned());
        }
    }
    None
}

fn parse_docker_size(value: &str) -> Option<u64> {
    let split = value.find(|character: char| {
        !(character.is_ascii_digit() || character == '.' || character == ',')
    })?;
    let number = value[..split]
        .trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()?;
    let suffix = value[split..].trim().to_ascii_lowercase();
    let multiplier = match suffix.as_str() {
        "b" => 1_f64,
        "kb" => 1_000_f64,
        "kib" => 1_024_f64,
        "mb" => 1_000_000_f64,
        "mib" => 1_048_576_f64,
        "gb" => 1_000_000_000_f64,
        "gib" => 1_073_741_824_f64,
        "tb" => 1_000_000_000_000_f64,
        "tib" => 1_099_511_627_776_f64,
        _ => return None,
    };
    let bytes = number * multiplier;
    (bytes.is_finite() && bytes >= 0.0 && bytes <= u64::MAX as f64).then_some(bytes.round() as u64)
}

pub(super) fn safe_package(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".+_:@/-".contains(character))
        && !value.starts_with('-')
}

pub fn is_safe_docker_container_id(value: &str) -> bool {
    (12..=64).contains(&value.len()) && value.chars().all(|character| character.is_ascii_hexdigit())
}

pub(super) fn safe_detail(value: &str) -> String {
    value.replace(['\r', '\n'], " ").chars().take(240).collect()
}

pub(super) fn parse_dpkg_packages(output: &str) -> Vec<AgentInstalledPackage> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            Some(AgentInstalledPackage {
                name: fields.next()?.trim().to_owned(),
                installed_version: fields.next()?.trim().to_owned(),
                candidate_version: None,
                architecture: fields
                    .next()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned),
                source: Some("dpkg".to_owned()),
            })
            .filter(|package| !package.name.is_empty() && !package.installed_version.is_empty())
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowsPackageParseError {
    InvalidOutput,
}

pub fn parse_windows_packages(
    output: &str,
) -> Result<Vec<AgentInstalledPackage>, WindowsPackageParseError> {
    if output.trim().is_empty() {
        return Err(WindowsPackageParseError::InvalidOutput);
    }
    let mut packages = Vec::new();
    let mut has_table_separator = false;
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.len() >= 3 && trimmed.bytes().all(|byte| byte == b'-') {
            has_table_separator = true;
            continue;
        }
        let columns = split_winget_columns(line);
        if !has_table_separator || columns.len() < 4 {
            continue;
        }
        if !safe_winget_id(columns[1]) {
            return Err(WindowsPackageParseError::InvalidOutput);
        }
        if columns[0].is_empty()
            || columns[0].len() > 256
            || columns[2].is_empty()
            || columns[2].len() > 128
        {
            return Err(WindowsPackageParseError::InvalidOutput);
        }
        let candidate_version = (columns.len() >= 5)
            .then(|| columns[3].trim())
            .filter(|value| !value.is_empty() && !matches!(*value, "-" | "Unknown" | "N/A"))
            .filter(|value| value.len() <= 128)
            .map(str::to_owned);
        let source = columns
            .last()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty() && value.len() <= 128)
            .map(str::to_owned);
        packages.push(AgentInstalledPackage {
            // The stable package identifier is the safe selector used by
            // policy and exact winget upgrade operations.
            name: columns[1].to_owned(),
            installed_version: columns[2].to_owned(),
            candidate_version,
            architecture: None,
            source,
        });
        if packages.len() > 50_000 {
            return Err(WindowsPackageParseError::InvalidOutput);
        }
    }
    if !has_table_separator {
        return Err(WindowsPackageParseError::InvalidOutput);
    }
    Ok(packages)
}

fn split_winget_columns(line: &str) -> Vec<&str> {
    let mut columns = Vec::new();
    let mut start = None;
    let mut whitespace = 0;
    for (index, character) in line.char_indices() {
        if character.is_whitespace() {
            whitespace += character.len_utf8();
            if whitespace >= 2 {
                if let Some(column_start) = start.take() {
                    columns.push(
                        line[column_start..index - (whitespace - character.len_utf8())].trim(),
                    );
                }
            }
        } else {
            if start.is_none() {
                start = Some(index);
            }
            whitespace = 0;
        }
    }
    if let Some(column_start) = start {
        columns.push(line[column_start..].trim());
    }
    columns
}

pub fn safe_winget_id(value: &str) -> bool {
    if value.is_empty() || value.len() > 128 || value.starts_with('-') {
        return false;
    }
    let mut segment_has_character = false;
    for character in value.chars() {
        match character {
            '.' if segment_has_character => segment_has_character = false,
            character if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') => {
                segment_has_character = true;
            }
            _ => return false,
        }
    }
    segment_has_character && value.contains('.')
}

pub(super) fn parse_docker_containers(output: &str) -> Vec<DockerContainerInfo> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(7, '\t');
            let id = safe_docker_container_id(fields.next()?)?;
            let name = fields.next()?.to_owned();
            let image = fields.next()?.to_owned();
            let state = fields.next()?.to_owned();
            let status = fields.next()?.to_owned();
            let ports = fields
                .next()
                .unwrap_or_default()
                .split(", ")
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect();
            let labels = sanitize_docker_labels(fields.next().unwrap_or_default());
            Some(DockerContainerInfo {
                id,
                name,
                image,
                state,
                status,
                ports,
                started_at: None,
                created_at: None,
                image_id: None,
                restart_count: None,
                health: None,
                oom_killed: None,
                labels,
            })
        })
        .collect()
}

pub(super) fn parse_docker_inspect_metadata(
    output: &str,
) -> HashMap<String, DockerInspectMetadata> {
    let Ok(entries) = serde_json::from_str::<Vec<serde_json::Value>>(output) else {
        return HashMap::new();
    };
    entries
        .into_iter()
        .filter_map(|entry| {
            let id = entry.get("Id")?.as_str()?;
            let id = safe_docker_container_id(id)?;
            let state = entry.get("State");
            let image_id = entry
                .get("Image")
                .and_then(serde_json::Value::as_str)
                .and_then(safe_image_id);
            let created_at = safe_docker_timestamp(entry.get("Created"));
            let started_at = state.and_then(|value| safe_docker_timestamp(value.get("StartedAt")));
            let health = state
                .and_then(|value| value.get("Health"))
                .and_then(|value| value.get("Status"))
                .and_then(serde_json::Value::as_str)
                .filter(|value| matches!(*value, "starting" | "healthy" | "unhealthy"))
                .map(str::to_owned);
            let restart_count = entry
                .get("RestartCount")
                .and_then(serde_json::Value::as_u64);
            Some((
                id,
                DockerInspectMetadata {
                    created_at,
                    image_id,
                    restart_count,
                    health,
                    oom_killed: state
                        .and_then(|value| value.get("OOMKilled"))
                        .and_then(serde_json::Value::as_bool),
                    started_at,
                },
            ))
        })
        .collect()
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct DockerInspectMetadata {
    pub created_at: Option<String>,
    pub image_id: Option<String>,
    pub restart_count: Option<u64>,
    pub health: Option<String>,
    pub oom_killed: Option<bool>,
    pub started_at: Option<String>,
}

fn safe_docker_container_id(value: &str) -> Option<String> {
    (12..=64)
        .contains(&value.len())
        .then_some(value)
        .filter(|value| value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(str::to_owned)
}

fn safe_image_id(value: &str) -> Option<String> {
    let digest = value.strip_prefix("sha256:")?;
    (digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| value.to_owned())
}

fn safe_docker_timestamp(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 64)
        .map(str::to_owned)
}

pub(super) fn docker_failure_reason(
    stderr: &[u8],
    spawn_error: Option<std::io::ErrorKind>,
) -> &'static str {
    if String::from_utf8_lossy(stderr)
        .to_ascii_lowercase()
        .contains("permission denied")
    {
        return "docker_permission_denied";
    }
    if spawn_error == Some(std::io::ErrorKind::NotFound) {
        return "docker_cli_unavailable";
    }
    "docker_unavailable"
}

pub(super) fn sanitize_docker_labels(labels: &str) -> Vec<String> {
    labels
        .split(',')
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .filter(|label| {
            let name = label
                .split_once('=')
                .map_or(*label, |(name, _)| name)
                .to_ascii_lowercase();
            !["secret", "token", "password", "credential", "private_key"]
                .iter()
                .any(|term| name.contains(term))
        })
        .map(str::to_owned)
        .collect()
}

pub(super) fn normalize_apt_list(output: &str) -> String {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with("Listing") {
                return None;
            }
            let (package, _) = line.split_once('/')?;
            let fields: Vec<_> = line.split_whitespace().collect();
            let candidate = fields.get(1)?;
            let marker = "[upgradable from: ";
            let installed = line.split(marker).nth(1)?.trim_end_matches(']');
            let security = line.contains("security");
            Some(format!("Package: {package}\nInstalled: {installed}\nCandidate: {candidate}\nSecurity: {}\nHeld: no\nAuthenticated: yes", if security { "yes" } else { "no" }))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}
