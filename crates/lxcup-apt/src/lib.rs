//! Safe APT inspection boundary and parser.

use lxcup_core::{
    AvailableUpdate, OperatingSystem, PackageName, PackageVersion, Scan, ScanStatus,
    UpdateClassification,
};
use thiserror::Error;

/// Read-only request for a package inventory inside one LXC.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AptInspectionRequest {
    pub container_id: u64,
    pub operating_system: OperatingSystem,
}

impl AptInspectionRequest {
    /// Returns fixed process arguments; no shell string is constructed.
    pub fn command_arguments(&self) -> Result<Vec<String>, AptError> {
        if self.container_id == 0 {
            return Err(AptError::InvalidContainerId);
        }
        match &self.operating_system {
            OperatingSystem::Debian | OperatingSystem::Ubuntu => {
                Ok(vec!["apt-cache".to_owned(), "policy".to_owned()])
            }
            OperatingSystem::Unknown(_) => Err(AptError::UnsupportedOperatingSystem),
        }
    }
}

/// Captured output from a controlled agent execution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AptExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

/// One normalized package record from the scanner contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AptPackageRecord {
    pub package: String,
    pub installed_version: String,
    pub candidate_version: Option<String>,
    pub classification: UpdateClassification,
    pub held: bool,
    pub authenticated: bool,
}

impl AptPackageRecord {
    pub fn into_available_update(self) -> Result<AvailableUpdate, AptParseError> {
        let candidate_version = self
            .candidate_version
            .ok_or(AptParseError::MissingField("Candidate"))?;
        Ok(AvailableUpdate {
            package: PackageName::new(self.package)
                .map_err(|_| AptParseError::InvalidPackageName)?,
            installed_version: PackageVersion::new(self.installed_version)
                .map_err(|_| AptParseError::InvalidPackageVersion)?,
            candidate_version: PackageVersion::new(candidate_version)
                .map_err(|_| AptParseError::InvalidPackageVersion)?,
            classification: self.classification,
            held: self.held,
        })
    }
}

/// A complete domain scan produced from one controlled APT execution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AptScanResult {
    pub scan: Scan,
    pub diagnostics: String,
}

pub fn build_scan(
    container_id: lxcup_core::ContainerId,
    execution: AptExecutionResult,
) -> Result<AptScanResult, AptError> {
    let mut scan = Scan::new(container_id);
    scan.transition_to(ScanStatus::Running, chrono::Utc::now())
        .expect("new scan can enter running");

    if execution.exit_code != Some(0) {
        scan.transition_to(ScanStatus::Failed, chrono::Utc::now())
            .expect("running scan can fail");
        return Ok(AptScanResult {
            scan,
            diagnostics: execution.stderr,
        });
    }

    scan.updates = parse_output(&execution.stdout)?
        .into_iter()
        .map(AptPackageRecord::into_available_update)
        .collect::<Result<_, _>>()?;
    scan.transition_to(ScanStatus::Succeeded, chrono::Utc::now())
        .expect("running scan can succeed");
    Ok(AptScanResult {
        scan,
        diagnostics: execution.stderr,
    })
}

/// Parses the line-oriented, machine-readable output produced by an agent.
/// Records are separated by blank lines and use `Key: value` fields.
pub fn parse_output(output: &str) -> Result<Vec<AptPackageRecord>, AptParseError> {
    let mut records = Vec::new();
    for block in output
        .split("\n\n")
        .filter(|block| !block.trim().is_empty())
    {
        let mut package = None;
        let mut installed = None;
        let mut candidate = None;
        let mut security = false;
        let mut held = false;
        let mut authenticated = true;

        for line in block.lines() {
            let (key, value) = line
                .split_once(':')
                .ok_or_else(|| AptParseError::MalformedLine(line.to_owned()))?;
            let value = value.trim();
            match key.trim() {
                "Package" => package = Some(value.to_owned()),
                "Installed" => installed = Some(value.to_owned()),
                "Candidate" => candidate = (!value.is_empty()).then(|| value.to_owned()),
                "Security" => security = parse_bool(value)?,
                "Held" => held = parse_bool(value)?,
                "Authenticated" => authenticated = parse_bool(value)?,
                _ => {}
            }
        }

        let package = package
            .filter(|value| !value.is_empty())
            .ok_or(AptParseError::MissingField("Package"))?;
        let installed_version = installed
            .filter(|value| !value.is_empty())
            .ok_or(AptParseError::MissingField("Installed"))?;
        let classification = if !authenticated || candidate.is_none() {
            UpdateClassification::Unknown
        } else if security {
            UpdateClassification::Security
        } else {
            UpdateClassification::Normal
        };

        records.push(AptPackageRecord {
            package,
            installed_version,
            candidate_version: candidate,
            classification,
            held,
            authenticated,
        });
    }
    Ok(records)
}

fn parse_bool(value: &str) -> Result<bool, AptParseError> {
    match value.to_ascii_lowercase().as_str() {
        "yes" | "true" | "1" => Ok(true),
        "no" | "false" | "0" => Ok(false),
        _ => Err(AptParseError::InvalidBoolean(value.to_owned())),
    }
}

#[derive(Debug, Error)]
pub enum AptError {
    #[error("container id must be greater than zero")]
    InvalidContainerId,
    #[error("the container operating system is not supported by the APT scanner")]
    UnsupportedOperatingSystem,
    #[error("APT output could not be parsed")]
    Parse(#[from] AptParseError),
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum AptParseError {
    #[error("malformed APT output line: {0}")]
    MalformedLine(String),
    #[error("APT output is missing the {0} field")]
    MissingField(&'static str),
    #[error("invalid boolean value in APT output: {0}")]
    InvalidBoolean(String),
    #[error("APT output contains an invalid package name")]
    InvalidPackageName,
    #[error("APT output contains an invalid package version")]
    InvalidPackageVersion,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_arguments_are_fixed_and_os_checked() {
        let request = AptInspectionRequest {
            container_id: 101,
            operating_system: OperatingSystem::Debian,
        };
        assert_eq!(
            request.command_arguments().unwrap(),
            ["apt-cache", "policy"]
        );
        assert!(
            AptInspectionRequest {
                container_id: 101,
                operating_system: OperatingSystem::Unknown("alpine".to_owned()),
            }
            .command_arguments()
            .is_err()
        );
    }

    #[test]
    fn parser_classifies_security_held_and_unknown_records() {
        let output = "Package: openssl\nInstalled: 1.0\nCandidate: 1.1\nSecurity: yes\nHeld: no\nAuthenticated: yes\n\nPackage: nginx\nInstalled: 2.0\nCandidate: 2.1\nSecurity: no\nHeld: yes\nAuthenticated: yes\n\nPackage: local\nInstalled: 1.0\nCandidate:\nSecurity: no\nHeld: no\nAuthenticated: no";
        let records = parse_output(output).unwrap();

        assert_eq!(records.len(), 3);
        assert_eq!(records[0].classification, UpdateClassification::Security);
        assert!(records[1].held);
        assert_eq!(records[2].classification, UpdateClassification::Unknown);
    }

    #[test]
    fn parser_rejects_malformed_records() {
        assert!(matches!(
            parse_output("Package openssl"),
            Err(AptParseError::MalformedLine(_))
        ));
        assert!(matches!(
            parse_output("Package: openssl\nInstalled: 1\nSecurity: maybe"),
            Err(AptParseError::InvalidBoolean(_))
        ));
    }

    #[test]
    fn command_arguments_reject_zero_and_unknown_operating_systems() {
        assert!(matches!(
            AptInspectionRequest {
                container_id: 0,
                operating_system: OperatingSystem::Ubuntu,
            }
            .command_arguments(),
            Err(AptError::InvalidContainerId)
        ));
        assert!(matches!(
            AptInspectionRequest {
                container_id: 5,
                operating_system: OperatingSystem::Unknown("other".to_owned()),
            }
            .command_arguments(),
            Err(AptError::UnsupportedOperatingSystem)
        ));
        assert_eq!(
            AptInspectionRequest {
                container_id: 5,
                operating_system: OperatingSystem::Ubuntu,
            }
            .command_arguments()
            .unwrap(),
            ["apt-cache", "policy"]
        );
    }

    #[test]
    fn parser_handles_empty_optional_fields_and_rejects_missing_required_fields() {
        assert!(parse_output("  \n\n").unwrap().is_empty());
        assert_eq!(
            parse_output("Package: app\nInstalled: 1\nCandidate: 2\nSecurity: TRUE\nHeld: 1\nAuthenticated: 0")
                .unwrap()[0]
                .classification,
            UpdateClassification::Unknown
        );
        assert!(matches!(
            parse_output("Package: app"),
            Err(AptParseError::MissingField("Installed"))
        ));
        assert!(matches!(
            parse_output("Installed: 1"),
            Err(AptParseError::MissingField("Package"))
        ));
        assert!(matches!(
            parse_output("Package: app\nInstalled: 1\nSecurity: yesish"),
            Err(AptParseError::InvalidBoolean(value)) if value == "yesish"
        ));
    }

    #[test]
    fn records_and_scans_convert_success_failure_and_invalid_output() {
        let parsed = parse_output(
            "Package: curl\nInstalled: 8.0\nCandidate: 8.1\nSecurity: no\nHeld: yes\nAuthenticated: yes",
        )
        .unwrap()
        .remove(0)
        .into_available_update()
        .unwrap();
        assert_eq!(parsed.package.as_str(), "curl");
        assert!(parsed.held);

        let id = lxcup_core::ContainerId::new(37);
        let success = build_scan(
            id,
            AptExecutionResult {
                stdout: "Package: curl\nInstalled: 8.0\nCandidate: 8.1\nSecurity: yes\nHeld: no\nAuthenticated: yes".to_owned(),
                stderr: "warning".to_owned(),
                exit_code: Some(0),
            },
        )
        .unwrap();
        assert_eq!(success.scan.status, ScanStatus::Succeeded);
        assert_eq!(success.scan.updates.len(), 1);
        assert_eq!(success.diagnostics, "warning");

        let failure = build_scan(
            id,
            AptExecutionResult {
                stdout: String::new(),
                stderr: "apt failed".to_owned(),
                exit_code: Some(100),
            },
        )
        .unwrap();
        assert_eq!(failure.scan.status, ScanStatus::Failed);
        assert_eq!(failure.diagnostics, "apt failed");

        assert!(matches!(
            build_scan(
                id,
                AptExecutionResult {
                    stdout: "Package: bad name\nInstalled: 1.0\nCandidate: 2.0".to_owned(),
                    stderr: String::new(),
                    exit_code: Some(0),
                }
            ),
            Err(AptError::Parse(AptParseError::InvalidPackageName))
        ));
    }
}
