use super::{AgentInstalledPackage, DockerContainerInfo};

pub(super) fn safe_package(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".+_:@/-".contains(character))
        && !value.starts_with('-')
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

pub(super) fn parse_windows_packages(output: &str) -> Vec<AgentInstalledPackage> {
    let value: serde_json::Value = match serde_json::from_str(output) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    let entries = match value {
        serde_json::Value::Array(entries) => entries,
        entry => vec![entry],
    };
    entries
        .into_iter()
        .filter_map(|entry| {
            Some(AgentInstalledPackage {
                name: entry.get("Name")?.as_str()?.to_owned(),
                installed_version: entry.get("Version")?.as_str()?.to_owned(),
                architecture: None,
                source: entry
                    .get("ProviderName")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect()
}

pub(super) fn parse_docker_containers(output: &str) -> Vec<DockerContainerInfo> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(8, '\t');
            Some(DockerContainerInfo {
                id: fields.next()?.to_owned(),
                name: fields.next()?.to_owned(),
                image: fields.next()?.to_owned(),
                state: fields.next()?.to_owned(),
                status: fields.next()?.to_owned(),
                ports: fields
                    .next()
                    .unwrap_or_default()
                    .split(", ")
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect(),
                started_at: fields
                    .next()
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned),
                labels: sanitize_docker_labels(fields.next().unwrap_or_default()),
            })
        })
        .collect()
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
