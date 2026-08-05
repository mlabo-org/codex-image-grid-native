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
    let content_type = request.headers().get(header::CONTENT_TYPE).cloned();
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
    if !content_type.as_ref().is_some_and(is_json_content_type) {
        return Err(JsonBodyError {
            status: StatusCode::UNSUPPORTED_MEDIA_TYPE,
            code: "UnsupportedContentType",
            message: "non-empty request bodies require Content-Type: application/json".to_owned(),
        });
    }
    serde_json::from_slice(&bytes).map_err(|error| JsonBodyError {
        status: StatusCode::BAD_REQUEST,
        code: "InvalidJsonBody",
        message: format!("invalid JSON request body: {error}"),
    })
}

fn is_json_content_type(value: &axum::http::HeaderValue) -> bool {
    value.to_str().ok().is_some_and(|value| {
        value
            .split(';')
            .next()
            .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("application/json"))
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
                .header(header::CONTENT_TYPE, "application/json")
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

        let unsupported = read_json_body(
            Request::builder()
                .body(Body::from("{}"))
                .expect("unsupported request"),
        )
        .await
        .expect_err("non-empty body without JSON content type must be rejected");
        assert_eq!(unsupported.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(unsupported.code, "UnsupportedContentType");
    }
}
