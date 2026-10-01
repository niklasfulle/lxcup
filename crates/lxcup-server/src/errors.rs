use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize)]
pub struct ApiErrorBody {
    pub error: ApiErrorInfo,
    pub request_id: Uuid,
}

#[derive(Clone, Debug, Serialize)]
pub struct ApiErrorInfo {
    pub code: &'static str,
    pub message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocking_job: Option<BlockingJobInfo>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BlockingJobInfo {
    pub id: Uuid,
    pub status: &'static str,
}

#[derive(Clone, Debug)]
pub struct ApiError {
    pub(crate) status: StatusCode,
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
    pub(crate) blocking_job: Option<BlockingJobInfo>,
}

impl ApiError {
    pub(crate) fn bad_request(code: &'static str, message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            message,
            blocking_job: None,
        }
    }

    pub(crate) fn conflict_with_blocking_job(
        code: &'static str,
        message: &'static str,
        id: Uuid,
        status: &'static str,
    ) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code,
            message,
            blocking_job: Some(BlockingJobInfo { id, status }),
        }
    }

    pub(crate) fn conflict(code: &'static str, message: &'static str) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code,
            message,
            blocking_job: None,
        }
    }

    pub(crate) fn not_found(resource: &'static str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: resource,
            blocking_job: None,
        }
    }

    pub(crate) fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: "authentication is required",
            blocking_job: None,
        }
    }

    pub(crate) fn forbidden(code: &'static str, message: &'static str) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code,
            message,
            blocking_job: None,
        }
    }

    pub(crate) fn dependency(code: &'static str, message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            code,
            message,
            blocking_job: None,
        }
    }

    pub(crate) fn storage() -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "storage_error",
            message: "the operation could not be persisted",
            blocking_job: None,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ApiErrorBody {
            error: ApiErrorInfo {
                code: self.code,
                message: self.message,
                blocking_job: self.blocking_job,
            },
            request_id: Uuid::new_v4(),
        };
        (self.status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use serde_json::Value;

    #[tokio::test]
    async fn target_conflict_exposes_the_blocking_job_for_ui_navigation() {
        let id = Uuid::new_v4();
        let response = ApiError::conflict_with_blocking_job(
            "ansible_target_busy",
            "another job is active for this target",
            id,
            "reconcile_required",
        )
        .into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["blocking_job"]["id"], id.to_string());
        assert_eq!(
            body["error"]["blocking_job"]["status"],
            "reconcile_required"
        );
    }
}
