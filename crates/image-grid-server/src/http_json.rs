use axum::Json;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use image_grid_core::MAX_REFERENCE_IMAGE_BYTES;
use serde_json::{Map, Value, json};
use std::env;

const MIN_JSON_BODY_BYTES: usize = 1024;
const MAX_JSON_BODY_BYTES: usize = 256 * 1024 * 1024;
pub(crate) const DEFAULT_JSON_BODY_BYTES: usize =
    ((MAX_REFERENCE_IMAGE_BYTES as usize * 4 + 2) / 3) + 1024 * 1024;

#[derive(Debug)]
pub(crate) struct JsonBodyError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl IntoResponse for JsonBodyError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({
                "error": self.message,
                "code": self.code
            })),
        )
            .into_response()
    }
}

pub(crate) async fn read_json_body(request: Request<Body>) -> Result<Value, JsonBodyError> {
    let limit = configured_json_body_limit();
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > limit)
    {
        return Err(body_too_large(limit));
    }

    let bytes = to_bytes(request.into_body(), limit)
        .await
        .map_err(|_| body_too_large(limit))?;
    if bytes.is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_slice(&bytes).map_err(|error| JsonBodyError {
        status: StatusCode::BAD_REQUEST,
        code: "InvalidJsonBody",
        message: format!("invalid JSON request body: {error}"),
    })
}

fn configured_json_body_limit() -> usize {
    env::var("IMAGE_GRID_MAX_JSON_BODY_BYTES")
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .map(|value| {
            value
                .round()
                .clamp(MIN_JSON_BODY_BYTES as f64, MAX_JSON_BODY_BYTES as f64) as usize
        })
        .unwrap_or(DEFAULT_JSON_BODY_BYTES)
}

fn body_too_large(limit: usize) -> JsonBodyError {
    JsonBodyError {
        status: StatusCode::PAYLOAD_TOO_LARGE,
        code: "RequestBodyTooLarge",
        message: format!("JSON request body exceeds the {limit} byte limit"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn malformed_and_declared_oversized_json_have_compatible_errors() {
        let malformed = read_json_body(
            Request::builder()
                .body(Body::from("{"))
                .expect("malformed request"),
        )
        .await
        .expect_err("malformed JSON must be rejected");
        assert_eq!(malformed.status, StatusCode::BAD_REQUEST);
        assert_eq!(malformed.code, "InvalidJsonBody");

        let oversized = read_json_body(
            Request::builder()
                .header(header::CONTENT_LENGTH, MAX_JSON_BODY_BYTES + 1)
                .body(Body::empty())
                .expect("oversized request"),
        )
        .await
        .expect_err("over-limit content length must be rejected");
        assert_eq!(oversized.status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(oversized.code, "RequestBodyTooLarge");
    }
}
