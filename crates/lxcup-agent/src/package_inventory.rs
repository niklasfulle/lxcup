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
    const MAX_INVENTORY_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
    let (program, arguments) = inventory_command(platform);
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
    let mut packages = if platform == AgentPlatform::Windows {
        crate::parse_windows_packages(&String::from_utf8_lossy(&output))
            .map_err(|()| PackageInventoryError::InvalidOutput)?
    } else {
        crate::parse_dpkg_packages(&String::from_utf8_lossy(&output))
    };
    if platform == AgentPlatform::Windows {
        packages.retain(|package| package.candidate_version.is_some());
    }
    if packages.len() > 50_000 {
        return Err(PackageInventoryError::TooLarge);
    }
    Ok(AgentPackageInventory {
        collected_at: Utc::now(),
        packages,
    })
}

fn inventory_command(platform: AgentPlatform) -> (&'static str, Vec<&'static str>) {
    match platform {
        AgentPlatform::Windows => (
            "winget",
            vec![
                "list",
                "--upgrade-available",
                "--disable-interactivity",
                "--accept-source-agreements",
            ],
        ),
        AgentPlatform::Linux => (
            "dpkg-query",
            vec!["-W", "-f=${binary:Package}\t${Version}\t${Architecture}\\n"],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::inventory_command;
    use crate::AgentPlatform;

    #[test]
    fn windows_inventory_uses_only_local_winget_upgrade_listing() {
        assert_eq!(
            inventory_command(AgentPlatform::Windows),
            (
                "winget",
                vec![
                    "list",
                    "--upgrade-available",
                    "--disable-interactivity",
                    "--accept-source-agreements"
                ]
            )
        );
    }
}
