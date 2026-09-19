//! Transport layer for the Proxmox REST API.
//!
//! Endpoint-specific models and operations are intentionally kept out of this
//! module. They can build on [`ProxmoxClient::get_json`] without exposing API
//! credentials to the domain or frontend layers.

use std::{fmt, net::SocketAddr, time::Duration};

use reqwest::{Client, StatusCode, header};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;
use url::Url;

pub mod endpoints;
pub mod models;
pub mod node;
pub mod polling;

const API_ROOT_PATH: &str = "api2/json/";

/// Configuration required to connect to a Proxmox API endpoint.
#[derive(Clone)]
pub struct ProxmoxClientConfig {
    base_url: Url,
    token_id: String,
    token_secret: String,
    timeout: Duration,
    root_certificate_pem: Option<Vec<u8>>,
    connect_override: Option<(String, SocketAddr)>,
}

impl ProxmoxClientConfig {
    /// Creates a configuration that only permits HTTPS endpoints.
    pub fn new(
        base_url: impl AsRef<str>,
        token_id: impl Into<String>,
        token_secret: impl Into<String>,
    ) -> Result<Self, ProxmoxConfigError> {
        let base_url = Url::parse(base_url.as_ref())?;
        if base_url.scheme() != "https" {
            return Err(ProxmoxConfigError::InsecureBaseUrl);
        }
        if base_url.host_str().is_none() {
            return Err(ProxmoxConfigError::MissingHost);
        }

        let token_id = token_id.into();
        if token_id.trim().is_empty() {
            return Err(ProxmoxConfigError::EmptyTokenId);
        }

        let token_secret = token_secret.into();
        if token_secret.is_empty() {
            return Err(ProxmoxConfigError::EmptyTokenSecret);
        }

        Ok(Self {
            base_url,
            token_id,
            token_secret,
            timeout: Duration::from_secs(30),
            root_certificate_pem: None,
            connect_override: None,
        })
    }

    /// Sets the upper bound for a single HTTP request.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Adds a PEM-encoded CA certificate for private Proxmox installations.
    #[must_use]
    pub fn with_root_certificate_pem(mut self, certificate_pem: impl AsRef<[u8]>) -> Self {
        self.root_certificate_pem = Some(certificate_pem.as_ref().to_vec());
        self
    }

    /// Routes a TLS hostname to a fixed socket address without changing the
    /// hostname used for certificate validation and SNI.
    #[must_use]
    pub fn with_connect_override(
        mut self,
        hostname: impl Into<String>,
        address: SocketAddr,
    ) -> Self {
        self.connect_override = Some((hostname.into(), address));
        self
    }
}

impl fmt::Debug for ProxmoxClientConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProxmoxClientConfig")
            .field("base_url", &self.base_url)
            .field("token_id", &self.token_id)
            .field("token_secret", &"[REDACTED]")
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// A configured client for read and task endpoints of the Proxmox API.
pub struct ProxmoxClient {
    http: Client,
    api_root: Url,
    token_id: String,
    token_secret: String,
}

impl fmt::Debug for ProxmoxClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProxmoxClient")
            .field("api_root", &self.api_root)
            .field("token_id", &self.token_id)
            .field("token_secret", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl ProxmoxClient {
    /// Builds a client with rustls certificate validation and request timeout.
    pub fn new(config: ProxmoxClientConfig) -> Result<Self, ProxmoxClientError> {
        let api_root = config
            .base_url
            .join(API_ROOT_PATH)
            .map_err(ProxmoxClientError::InvalidUrl)?;
        let mut client_builder = Client::builder().use_rustls_tls().timeout(config.timeout);
        if let Some(certificate_pem) = config.root_certificate_pem.as_deref() {
            let certificate = reqwest::Certificate::from_pem(certificate_pem)
                .map_err(ProxmoxClientError::HttpClient)?;
            client_builder = client_builder.add_root_certificate(certificate);
        }
        if let Some((hostname, address)) = config.connect_override {
            client_builder = client_builder.resolve(&hostname, address);
        }
        let http = client_builder
            .build()
            .map_err(ProxmoxClientError::HttpClient)?;

        Ok(Self {
            http,
            api_root,
            token_id: config.token_id,
            token_secret: config.token_secret,
        })
    }

    /// Performs an authenticated GET request and unwraps Proxmox's `data` envelope.
    pub async fn get_json<T>(&self, path: &str) -> Result<T, ProxmoxClientError>
    where
        T: DeserializeOwned,
    {
        let relative_path = path.trim_start_matches('/');
        if relative_path.is_empty() {
            return Err(ProxmoxClientError::InvalidPath);
        }

        let url = self
            .api_root
            .join(relative_path)
            .map_err(ProxmoxClientError::InvalidUrl)?;
        let authorization = format!("PVEAPIToken={}={}", self.token_id, self.token_secret);
        let response = self
            .http
            .get(url)
            .header(header::AUTHORIZATION, authorization)
            .send()
            .await
            .map_err(ProxmoxClientError::Transport)?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(ProxmoxClientError::Transport)?;

        if !status.is_success() {
            return Err(ProxmoxClientError::Api {
                status,
                message: api_error_message(&body),
            });
        }

        let envelope = serde_json::from_str::<ApiResponse<T>>(&body)
            .map_err(|source| ProxmoxClientError::Decode { source })?;
        Ok(envelope.data)
    }

    /// Performs an authenticated POST and unwraps Proxmox's `data` envelope.
    pub async fn post_json<T, B>(&self, path: &str, body: &B) -> Result<T, ProxmoxClientError>
    where
        T: DeserializeOwned,
        B: Serialize,
    {
        let relative_path = path.trim_start_matches('/');
        if relative_path.is_empty() {
            return Err(ProxmoxClientError::InvalidPath);
        }
        let url = self
            .api_root
            .join(relative_path)
            .map_err(ProxmoxClientError::InvalidUrl)?;
        let authorization = format!("PVEAPIToken={}={}", self.token_id, self.token_secret);
        let response = self
            .http
            .post(url)
            .header(header::AUTHORIZATION, authorization)
            .json(body)
            .send()
            .await
            .map_err(ProxmoxClientError::Transport)?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(ProxmoxClientError::Transport)?;
        if !status.is_success() {
            return Err(ProxmoxClientError::Api {
                status,
                message: api_error_message(&body),
            });
        }
        let envelope = serde_json::from_str::<ApiResponse<T>>(&body)
            .map_err(|source| ProxmoxClientError::Decode { source })?;
        Ok(envelope.data)
    }
}

/// Errors while validating client configuration.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProxmoxConfigError {
    #[error("the Proxmox base URL is invalid: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("the Proxmox base URL must use HTTPS")]
    InsecureBaseUrl,
    #[error("the Proxmox base URL must contain a host")]
    MissingHost,
    #[error("the Proxmox API token id must not be empty")]
    EmptyTokenId,
    #[error("the Proxmox API token secret must not be empty")]
    EmptyTokenSecret,
}

/// Errors returned by the Proxmox transport.
#[derive(Debug, Error)]
pub enum ProxmoxClientError {
    #[error("the Proxmox URL is invalid: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("the HTTP client could not be built: {0}")]
    HttpClient(#[source] reqwest::Error),
    #[error("the Proxmox request failed: {0}")]
    Transport(#[source] reqwest::Error),
    #[error("the Proxmox API returned HTTP {status}: {message}")]
    Api { status: StatusCode, message: String },
    #[error("the Proxmox API returned invalid JSON: {source}")]
    Decode {
        #[source]
        source: serde_json::Error,
    },
    #[error("the Proxmox API path must not be empty")]
    InvalidPath,
}

#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct ApiErrorResponse {
    message: Option<String>,
    errors: Option<serde_json::Value>,
}

fn api_error_message(body: &str) -> String {
    serde_json::from_str::<ApiErrorResponse>(body)
        .ok()
        .and_then(|response| {
            response
                .message
                .or_else(|| response.errors.map(|errors| errors.to_string()))
        })
        .unwrap_or_else(|| "the Proxmox API returned an error".to_owned())
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        time::sleep,
    };

    use super::*;

    #[derive(Debug, Deserialize, PartialEq)]
    struct TestData {
        ok: bool,
    }

    async fn response_server(
        response_body: &str,
        status: &str,
        delay: Duration,
    ) -> (SocketAddr, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
            response_body.len()
        );
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let size = stream.read(&mut request).await.unwrap();
            sleep(delay).await;
            let _ = stream.write_all(response.as_bytes()).await;
            String::from_utf8_lossy(&request[..size]).into_owned()
        });
        (address, handle)
    }

    fn test_client(address: SocketAddr, timeout: Duration) -> ProxmoxClient {
        ProxmoxClient {
            http: Client::builder().timeout(timeout).build().unwrap(),
            api_root: Url::parse(&format!("http://{address}/api2/json/")).unwrap(),
            token_id: "lxcup-test@pam!transport".to_owned(),
            token_secret: "unit-test-token".to_owned(),
        }
    }

    #[test]
    fn config_requires_https_and_redacts_token_secret() {
        assert!(matches!(
            ProxmoxClientConfig::new("http://pve.example:8006", "user!token", "unit-test-token"),
            Err(ProxmoxConfigError::InsecureBaseUrl)
        ));

        let config =
            ProxmoxClientConfig::new("https://pve.example:8006", "user!token", "unit-test-token")
                .unwrap();
        assert!(!format!("{config:?}").contains("unit-test-token"));
    }

    #[test]
    fn connect_override_builds_a_client_without_disabling_tls_validation() {
        let config = ProxmoxClientConfig::new("https://pve:8006", "user!token", "unit-test-token")
            .unwrap()
            .with_connect_override("pve", "192.168.1.150:8006".parse().unwrap());

        ProxmoxClient::new(config).expect("connect override is a valid client configuration");
    }

    #[tokio::test]
    async fn authenticated_get_parses_data_envelope() {
        let (address, server) =
            response_server(r#"{"data":{"ok":true}}"#, "200 OK", Duration::ZERO).await;
        let client = test_client(address, Duration::from_secs(1));

        let result = client.get_json::<TestData>("/nodes").await.unwrap();
        let request = server.await.unwrap();

        assert_eq!(result, TestData { ok: true });
        assert!(request.contains("GET /api2/json/nodes HTTP/1.1"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: pveapitoken=lxcup-test@pam!transport=unit-test-token")
        );
    }

    #[tokio::test]
    async fn api_errors_are_structured_without_exposing_client_debug_secret() {
        let (address, server) = response_server(
            r#"{"message":"permission denied"}"#,
            "401 Unauthorized",
            Duration::ZERO,
        )
        .await;
        let client = test_client(address, Duration::from_secs(1));

        let error = client.get_json::<TestData>("/nodes").await.unwrap_err();
        let request = server.await.unwrap();

        match error {
            ProxmoxClientError::Api { status, message } => {
                assert_eq!(status, StatusCode::UNAUTHORIZED);
                assert_eq!(message, "permission denied");
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert!(request.contains("unit-test-token"));
        assert!(!format!("{client:?}").contains("unit-test-token"));
    }

    #[tokio::test]
    async fn request_timeout_is_enforced() {
        let (address, server) = response_server(
            r#"{"data":{"ok":true}}"#,
            "200 OK",
            Duration::from_millis(100),
        )
        .await;
        let client = test_client(address, Duration::from_millis(10));

        let error = client.get_json::<TestData>("/nodes").await.unwrap_err();
        let _ = server.await;

        match error {
            ProxmoxClientError::Transport(error) => assert!(error.is_timeout()),
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
