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

impl PackageInventoryError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::ManagerUnavailable => "package_manager_unavailable",
            Self::Timeout => "package_inventory_timeout",
            Self::TooLarge => "package_inventory_too_large",
            Self::InvalidOutput => "package_inventory_invalid",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "package_manager_unavailable" => Some(Self::ManagerUnavailable),
            "package_inventory_timeout" => Some(Self::Timeout),
            "package_inventory_too_large" => Some(Self::TooLarge),
            "package_inventory_invalid" => Some(Self::InvalidOutput),
            _ => None,
        }
    }
}

pub async fn collect_package_inventory(
    platform: AgentPlatform,
) -> Result<AgentPackageInventory, PackageInventoryError> {
    let packages = match platform {
        AgentPlatform::Windows => run_windows_inventory().await?,
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

async fn run_windows_inventory() -> Result<Vec<crate::AgentInstalledPackage>, PackageInventoryError>
{
    let output = run_inventory_command(
        crate::windows_powershell_program(),
        &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            windows_inventory_script(),
        ],
    )
    .await?;
    parse_windows_packages_json(&String::from_utf8_lossy(&output))
}

fn windows_inventory_script() -> &'static str {
    r#"$ErrorActionPreference = 'Stop'
Import-Module Microsoft.WinGet.Client -ErrorAction Stop
$packages = @(Get-WinGetPackage -ErrorAction Stop)
$inventory = @($packages | ForEach-Object {
    $installed = $_.InstalledVersion
    if ($null -ne $installed -and $null -ne $installed.PSObject.Properties['Version']) {
        $installed = $installed.Version
    }
    if ([string]::IsNullOrWhiteSpace([string]$installed)) {
        $installed = $_.Version
    }
    $candidate = $null
    if ($_.IsUpdateAvailable -and @($_.AvailableVersions).Count -gt 0) {
        $candidate = [string]@($_.AvailableVersions)[0]
    }
    [ordered]@{
        id = [string]$_.Id
        name = [string]$_.Name
        version = [string]$installed
        candidate_version = $candidate
        source = if ($null -ne $_.Source -and [string]$_.Source) { [string]$_.Source } else { $null }
    }
})
ConvertTo-Json -InputObject $inventory -Compress -Depth 4"#
}

pub fn parse_windows_packages_json(
    output: &str,
) -> Result<Vec<crate::AgentInstalledPackage>, PackageInventoryError> {
    #[derive(serde::Deserialize)]
    struct WinGetPackage {
        id: Option<String>,
        name: Option<String>,
        version: Option<String>,
        candidate_version: Option<String>,
        source: Option<String>,
    }

    let entries = serde_json::from_str::<Vec<serde_json::Value>>(output)
        .map_err(|_| PackageInventoryError::InvalidOutput)?;
    if entries.len() > 50_000 {
        return Err(PackageInventoryError::TooLarge);
    }
    let reported_count = entries.len();
    let mut seen_ids = std::collections::HashSet::new();
    let mut packages = Vec::with_capacity(reported_count);
    for entry in entries {
        let Ok(package) = serde_json::from_value::<WinGetPackage>(entry) else {
            continue;
        };
        let (Some(id), Some(name), Some(version)) = (package.id, package.name, package.version)
        else {
            continue;
        };
        let reported_source = package
            .source
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase);
        let (source, candidate_version) = match reported_source.as_deref() {
            Some("winget") if crate::safe_winget_id(&id) => ("winget", package.candidate_version),
            Some("msstore") if safe_msstore_id(&id) => ("msstore", package.candidate_version),
            _ if safe_read_only_windows_package_id(&id, "arp") => ("arp", None),
            _ if safe_read_only_windows_package_id(&id, "msix") => ("msix", None),
            _ => continue,
        };
        if name.trim().is_empty()
            || name.len() > 256
            || version.trim().is_empty()
            || version.len() > 128
            || candidate_version
                .as_ref()
                .is_some_and(|value| value.trim().is_empty() || value.len() > 128)
            || !seen_ids.insert(id.to_ascii_lowercase())
        {
            continue;
        }
        packages.push(crate::AgentInstalledPackage {
            name: id,
            installed_version: version,
            candidate_version,
            architecture: None,
            source: Some(source.to_owned()),
        });
    }
    if reported_count > 0 && packages.is_empty() {
        return Err(PackageInventoryError::InvalidOutput);
    }
    Ok(packages)
}

pub fn safe_msstore_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+'))
}

pub fn safe_read_only_windows_package_id(value: &str, source: &str) -> bool {
    let prefix = match source {
        "arp" => "ARP\\",
        "msix" => "MSIX\\",
        _ => return false,
    };
    value.len() <= 128
        && value
            .get(..prefix.len())
            .is_some_and(|value_prefix| value_prefix.eq_ignore_ascii_case(prefix))
        && value[prefix.len()..].bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'\\' | b'_' | b'-' | b'.' | b'{' | b'}')
        })
        && value.len() > prefix.len()
}

async fn run_inventory_command(
    program: impl AsRef<std::ffi::OsStr>,
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

fn dpkg_inventory_arguments() -> Vec<&'static str> {
    vec!["-W", "-f=${binary:Package}\t${Version}\t${Architecture}\\n"]
}

#[cfg(test)]
mod tests {
    use super::{PackageInventoryError, parse_windows_packages_json};

    #[test]
    fn package_inventory_errors_have_stable_round_trip_codes() {
        for error in [
            PackageInventoryError::ManagerUnavailable,
            PackageInventoryError::Timeout,
            PackageInventoryError::TooLarge,
            PackageInventoryError::InvalidOutput,
        ] {
            assert_eq!(PackageInventoryError::from_code(error.code()), Some(error));
        }
        assert_eq!(PackageInventoryError::from_code("token=secret"), None);
    }

    #[test]
    fn windows_inventory_uses_local_winget_package_module() {
        let script = super::windows_inventory_script();
        assert_eq!(
            crate::windows_powershell_program_at(std::ffi::OsStr::new(r"C:\Program Files")),
            std::path::PathBuf::from(r"C:\Program Files\PowerShell\7\pwsh.exe")
        );
        assert!(script.contains("Import-Module Microsoft.WinGet.Client"));
        assert!(script.contains("Get-WinGetPackage"));
        assert!(script.contains("$_.InstalledVersion"));
        assert!(script.contains("$_.IsUpdateAvailable"));
        assert!(!script.contains("winget.exe"));
        assert!(!script.contains("winget list"));
    }

    #[test]
    fn windows_inventory_parses_structured_winget_client_output() {
        let packages = parse_windows_packages_json(
            r#"[{"id":"Microsoft.PowerToys","name":"PowerToys","version":"0.90.0","candidate_version":"0.91.0","source":"winget"}]"#,
        )
        .unwrap();

        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "Microsoft.PowerToys");
        assert_eq!(packages[0].installed_version, "0.90.0");
        assert_eq!(packages[0].candidate_version.as_deref(), Some("0.91.0"));
        assert_eq!(packages[0].source.as_deref(), Some("winget"));
    }

    #[test]
    fn windows_inventory_discards_unsupported_individual_package_rows() {
        let packages = parse_windows_packages_json(
            r#"[
                {"id":"Microsoft.PowerToys","name":"PowerToys","version":"0.90.0","candidate_version":null,"source":"winget"},
                {"id":"Contoso.Legacy","name":"Legacy App","version":"1.0","candidate_version":null,"source":"Chocolatey"}
            ]"#,
        )
        .expect("a valid package should survive an unsupported installed-app row");

        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "Microsoft.PowerToys");
    }

    #[test]
    fn windows_inventory_retains_uncorrelated_arp_and_msix_packages_as_read_only_entries() {
        let packages = parse_windows_packages_json(
            r#"[
                {"id":"ARP\\Machine\\{12345678-1234-1234-1234-123456789abc}","name":"Legacy App","version":"1.0","candidate_version":null,"source":null},
                {"id":"MSIX\\Microsoft.Paint_8wekyb3d8bbwe","name":"Paint","version":"11.0","candidate_version":null,"source":null}
            ]"#,
        )
        .expect("valid but uncorrelated installed software should remain visible");

        assert_eq!(packages.len(), 2);
        assert_eq!(packages[0].source.as_deref(), Some("arp"));
        assert_eq!(packages[1].source.as_deref(), Some("msix"));
        assert!(
            packages
                .iter()
                .all(|package| package.candidate_version.is_none())
        );
    }

    #[test]
    fn windows_inventory_keeps_valid_store_and_plus_qualified_catalog_ids() {
        let packages = parse_windows_packages_json(
            r#"[
                {"id":"9N0DX20HK701","name":"Store App","version":"1.0","candidate_version":null,"source":"msstore"},
                {"id":"Microsoft.VCRedist.2015+.x86","name":"Visual C++ Runtime","version":"14.0","candidate_version":null,"source":"winget"}
            ]"#,
        )
        .expect("valid identifiers from WinGet and Store must remain in inventory");

        assert_eq!(packages.len(), 2);
        assert_eq!(packages[0].source.as_deref(), Some("msstore"));
        assert_eq!(packages[1].source.as_deref(), Some("winget"));
    }

    #[test]
    fn windows_inventory_rejects_nonempty_output_when_every_row_is_discarded() {
        let result = parse_windows_packages_json(
            r#"[{"id":"unexpected-format","name":"Installed App","version":"1.0","source":"winget"}]"#,
        );

        assert_eq!(result, Err(PackageInventoryError::InvalidOutput));
    }

    #[test]
    fn windows_inventory_discards_rows_with_invalid_shapes_and_normalizes_source() {
        let packages = parse_windows_packages_json(
            r#"[
                {"id":"Contoso.App","name":"Contoso","version":"1.0","candidate_version":null,"source":"WINGET"},
                {"id":42,"name":"Invalid ID","version":"1.0","source":"winget"},
                {"id":"Microsoft.StoreOnly","name":"Store App","version":"2.0","source":"msstore"}
            ]"#,
        )
        .unwrap();

        assert_eq!(packages.len(), 2);
        assert_eq!(packages[0].source.as_deref(), Some("winget"));
        assert_eq!(packages[1].source.as_deref(), Some("msstore"));
    }
}
