use crate::{AgentPackageInventory, AgentPlatform};
use chrono::Utc;
use std::time::Duration;
use tokio::process::Command;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageInventoryError {
    ManagerUnavailable,
    Timeout,
    TooLarge,
    InvalidOutput,
}

pub async fn collect_package_inventory(
    platform: AgentPlatform,
) -> Result<AgentPackageInventory, PackageInventoryError> {
    let packages = match platform {
        AgentPlatform::Windows => {
            let installed = run_winget_list(false).await?;
            let updates = run_winget_list(true).await?;
            merge_winget_updates(installed, updates)
        }
        AgentPlatform::Linux => {
            let output = run_inventory_command("dpkg-query", &dpkg_inventory_arguments()).await?;
            crate::parse_dpkg_packages(&String::from_utf8_lossy(&output))
        }
    };
    if packages.len() > 50_000 {
        return Err(PackageInventoryError::TooLarge);
    }
    Ok(AgentPackageInventory {
        collected_at: Utc::now(),
        packages,
    })
}

async fn run_winget_list(
    upgrades_only: bool,
) -> Result<Vec<crate::AgentInstalledPackage>, PackageInventoryError> {
    let output = run_inventory_command("winget", &winget_list_arguments(upgrades_only)).await?;
    crate::parse_windows_packages(&String::from_utf8_lossy(&output))
        .map_err(|_| PackageInventoryError::InvalidOutput)
}

async fn run_inventory_command(
    program: &str,
    arguments: &[&str],
) -> Result<Vec<u8>, PackageInventoryError> {
    const MAX_INVENTORY_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
    let mut command = Command::new(program);
    command.args(arguments);
    let output = match tokio::time::timeout(Duration::from_secs(30), command.output()).await {
        Ok(Ok(output)) if output.status.success() => output.stdout,
        Ok(Ok(_)) | Ok(Err(_)) => return Err(PackageInventoryError::ManagerUnavailable),
        Err(_) => return Err(PackageInventoryError::Timeout),
    };
    if output.len() > MAX_INVENTORY_OUTPUT_BYTES {
        return Err(PackageInventoryError::TooLarge);
    }
    Ok(output)
}

fn winget_list_arguments(upgrades_only: bool) -> Vec<&'static str> {
    let mut arguments = vec!["list"];
    if upgrades_only {
        arguments.push("--upgrade-available");
    }
    arguments.extend(["--disable-interactivity", "--accept-source-agreements"]);
    arguments
}

fn dpkg_inventory_arguments() -> Vec<&'static str> {
    vec!["-W", "-f=${binary:Package}\t${Version}\t${Architecture}\\n"]
}

fn merge_winget_updates(
    mut installed: Vec<crate::AgentInstalledPackage>,
    updates: Vec<crate::AgentInstalledPackage>,
) -> Vec<crate::AgentInstalledPackage> {
    let candidates = updates
        .into_iter()
        .filter_map(|package| {
            package
                .candidate_version
                .map(|version| (package.name.to_ascii_lowercase(), version))
        })
        .collect::<std::collections::HashMap<_, _>>();
    for package in &mut installed {
        if let Some(candidate) = candidates.get(&package.name.to_ascii_lowercase()) {
            package.candidate_version = Some(candidate.clone());
            package.source = Some("winget".to_owned());
        }
    }
    installed
}

#[cfg(test)]
mod tests {
    use super::{merge_winget_updates, winget_list_arguments};
    use crate::AgentInstalledPackage;

    #[test]
    fn windows_inventory_uses_only_local_winget_upgrade_listing() {
        assert_eq!(
            winget_list_arguments(false),
            vec![
                "list",
                "--disable-interactivity",
                "--accept-source-agreements"
            ]
        );
        assert_eq!(
            winget_list_arguments(true),
            vec![
                "list",
                "--upgrade-available",
                "--disable-interactivity",
                "--accept-source-agreements"
            ]
        );
    }

    #[test]
    fn windows_inventory_merges_only_updates_found_by_winget_upgrade_listing() {
        let installed = AgentInstalledPackage {
            name: "Microsoft.App".to_owned(),
            installed_version: "1.0".to_owned(),
            candidate_version: None,
            architecture: None,
            source: Some("winget".to_owned()),
        };
        let update = AgentInstalledPackage {
            name: "microsoft.app".to_owned(),
            installed_version: "1.0".to_owned(),
            candidate_version: Some("2.0".to_owned()),
            architecture: None,
            source: Some("winget".to_owned()),
        };
        let merged = merge_winget_updates(vec![installed.clone()], vec![update]);
        assert_eq!(merged[0].candidate_version.as_deref(), Some("2.0"));
        assert_eq!(merged[0].name, "Microsoft.App");
    }
}
