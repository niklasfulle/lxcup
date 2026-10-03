use lxcup_ansible::JobFailureCode;
use lxcup_core::TargetKind;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

#[derive(Deserialize)]
pub(crate) struct Manifest {
    pub(crate) version: String,
    pub(crate) artifacts: Vec<Artifact>,
}

#[derive(Deserialize)]
pub(crate) struct Artifact {
    pub(crate) platform: String,
    pub(crate) file: String,
    pub(crate) sha256: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AgentArtifactPaths {
    pub(crate) linux_amd64: Option<String>,
    pub(crate) linux_arm64: Option<String>,
    pub(crate) windows_amd64: Option<String>,
}

pub(crate) async fn resolve(
    artifacts_url: &str,
    version: &str,
    target_kind: TargetKind,
    directory: &Path,
) -> Result<AgentArtifactPaths, JobFailureCode> {
    let base = format!("{artifacts_url}/agent/{version}");
    let manifest: Manifest = reqwest::get(format!("{base}/manifest.json"))
        .await
        .map_err(|_| JobFailureCode::WorkerUnavailable)?
        .error_for_status()
        .map_err(|_| JobFailureCode::WorkerUnavailable)?
        .json()
        .await
        .map_err(|_| JobFailureCode::PlaybookFailed)?;
    if manifest.version != version {
        return Err(JobFailureCode::PlaybookFailed);
    }

    if target_kind == TargetKind::WindowsServer {
        let windows = manifest
            .artifacts
            .iter()
            .find(|artifact| artifact.platform == "windows-amd64")
            .ok_or(JobFailureCode::PlaybookFailed)?;
        return Ok(AgentArtifactPaths {
            linux_amd64: None,
            linux_arm64: None,
            windows_amd64: Some(
                download(&base, windows, &directory.join("agent-windows-amd64.exe")).await?,
            ),
        });
    }

    let amd64 = manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.platform == "linux-amd64")
        .ok_or(JobFailureCode::PlaybookFailed)?;
    let amd64_path = download(&base, amd64, &directory.join("agent-linux-amd64")).await?;
    let arm64_path = if let Some(arm64) = manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.platform == "linux-arm64")
    {
        Some(download(&base, arm64, &directory.join("agent-linux-arm64")).await?)
    } else {
        None
    };
    Ok(AgentArtifactPaths {
        linux_amd64: Some(amd64_path),
        linux_arm64: arm64_path,
        windows_amd64: None,
    })
}

async fn download(
    base: &str,
    artifact: &Artifact,
    destination: &Path,
) -> Result<String, JobFailureCode> {
    if artifact.file.is_empty()
        || !artifact.file.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
    {
        return Err(JobFailureCode::PlaybookFailed);
    }
    let binary = reqwest::get(format!("{base}/{}", artifact.file))
        .await
        .map_err(|_| JobFailureCode::WorkerUnavailable)?
        .bytes()
        .await
        .map_err(|_| JobFailureCode::WorkerUnavailable)?;
    if format!("{:x}", Sha256::digest(&binary)) != artifact.sha256
        || !matches!(
            artifact.platform.as_str(),
            "linux-amd64" | "linux-arm64" | "windows-amd64"
        )
        || !agent_binary_matches_platform(&binary, &artifact.platform)
    {
        return Err(JobFailureCode::PlaybookFailed);
    }
    fs::write(destination, binary).map_err(|_| JobFailureCode::WorkerUnavailable)?;
    Ok(destination.display().to_string())
}

pub(crate) fn agent_binary_matches_platform(binary: &[u8], platform: &str) -> bool {
    if platform == "windows-amd64" {
        return windows_pe_is_amd64(binary);
    }
    if binary.len() < 20 || binary.get(..4) != Some(b"\x7fELF") || binary[4] != 2 || binary[5] != 1
    {
        return false;
    }
    let machine = u16::from_le_bytes([binary[18], binary[19]]);
    matches!(
        (platform, machine),
        ("linux-amd64", 62) | ("linux-arm64", 183)
    )
}

fn windows_pe_is_amd64(binary: &[u8]) -> bool {
    if binary.get(..2) != Some(b"MZ") || binary.len() < 64 {
        return false;
    }
    let Some(pe_offset) = binary
        .get(0x3c..0x40)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u32::from_le_bytes)
        .and_then(|offset| usize::try_from(offset).ok())
    else {
        return false;
    };
    let Some(header) = binary.get(pe_offset..pe_offset.saturating_add(26)) else {
        return false;
    };
    header.get(..4) == Some(b"PE\0\0")
        && header.get(4..6) == Some(&0x8664_u16.to_le_bytes())
        && header.get(24..26) == Some(&0x20b_u16.to_le_bytes())
}
