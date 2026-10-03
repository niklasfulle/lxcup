use super::*;
use tokio_tungstenite::{connect_async, tungstenite::Error as WebSocketError};

#[tokio::test]
async fn terminal_websocket_requires_authentication_before_upgrade() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, router(ApiState::new())).await;
    });
    let target_id = lxcup_core::TargetId::new().as_uuid();
    let error = connect_async(format!(
        "ws://{address}/api/v1/targets/{target_id}/terminal"
    ))
    .await
    .unwrap_err();
    server.abort();

    let WebSocketError::Http(response) = error else {
        panic!("expected an HTTP authorization failure before WebSocket upgrade");
    };
    assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
}
