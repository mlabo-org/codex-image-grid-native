//! The pay-as-you-go GPT Image route: the OpenAI Image API called directly
//! with an API key, and the choice between it and the Codex App Server route.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde_json::{Value, json};
use std::time::Duration;

/// Draws through the OpenAI Image API; every image is billed to the API key.
pub(crate) const API_ENGINE: &str = "openai-image-api";
/// Picks `app-server-image` or `openai-image-api` from the ChatGPT plan when a
/// run is admitted. A run never keeps this name: its record carries the engine
/// that was picked.
pub(crate) const AUTO_ENGINE: &str = "gpt-image";
/// The model a run draws with when the request names none.
pub(crate) const DEFAULT_MODEL: &str = "gpt-image-2.6-sunburst";
/// The longest model ID a request may name.
const MAX_MODEL_CHARS: usize = 64;
const QUALITY: &str = "medium";
pub(crate) const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
/// The Image API's `image_url` length limit for one input image.
const MAX_IMAGE_URL_CHARS: usize = 20_971_520;
pub(crate) const ATTEMPTS: u32 = 2;
const RETRY_DELAY: Duration = Duration::from_secs(10);
const RETRY_DELAY_MAX: Duration = Duration::from_secs(60);

/// Which GPT Image route a `gpt-image` run takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    AppServer,
    Api,
}

impl Route {
    pub(crate) const fn engine(self) -> &'static str {
        match self {
            Self::AppServer => "app-server-image",
            Self::Api => API_ENGINE,
        }
    }
}

/// Reads the route from an App Server `account/read` result. A paid ChatGPT
/// plan draws through the App Server; a free plan, an API-key login, and no
/// login draw through the API. A plan the App Server cannot name picks
/// neither, because guessing either way could bill the user or fail the run.
pub(crate) fn route_for_account(result: &Value) -> Result<Route, String> {
    let Some(account) = result.get("account").filter(|account| !account.is_null()) else {
        return Ok(Route::Api);
    };
    if account.get("type").and_then(Value::as_str) != Some("chatgpt") {
        return Ok(Route::Api);
    }
    match account.get("planType").and_then(Value::as_str) {
        Some("free") => Ok(Route::Api),
        Some("unknown") | None => Err(
            "Codex did not report a ChatGPT plan, so the gpt-image engine cannot choose a route"
                .to_owned(),
        ),
        Some(_) => Ok(Route::AppServer),
    }
}

/// A failed Image API call, already worded for the job and free of the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApiFailure {
    pub code: &'static str,
    pub message: String,
    pub status: Option<u16>,
    /// Set when the same request may succeed after this wait.
    pub retry_after: Option<Duration>,
}

impl ApiFailure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            status: None,
            retry_after: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ImageRequest {
    pub path: &'static str,
    pub body: Value,
}

/// Checks a model ID named by a request. Any ID made of letters, digits,
/// dots, dashes, and underscores is passed through unchanged, so a GPT Image
/// version released later works without a change here; whether the ID exists
/// is OpenAI's to answer when the job runs.
pub(crate) fn validate_model(model: &str) -> Result<(), String> {
    let valid = !model.is_empty()
        && model.len() <= MAX_MODEL_CHARS
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if valid {
        Ok(())
    } else {
        Err(format!(
            "model must be an OpenAI image model ID of at most {MAX_MODEL_CHARS} letters, digits, dots, dashes, or underscores (for example {DEFAULT_MODEL})"
        ))
    }
}

/// The pixel size sent for each aspect ratio. Both edges are multiples of 16
/// and the total stays inside the Image API's pixel range.
pub(crate) fn size_for_aspect_ratio(aspect_ratio: &str) -> &'static str {
    match aspect_ratio {
        "4:3" => "1408x1056",
        "1:1" => "1024x1024",
        "3:4" => "1056x1408",
        "9:16" => "864x1536",
        _ => "1536x864",
    }
}

/// Builds the Image API call for one job. A job with an image goes to the
/// edits endpoint; an edit keeps the source's own size, and a
/// reference-guided generation asks for the run's aspect ratio.
pub(crate) fn build_request(
    model: &str,
    prompt: &str,
    aspect_ratio: &str,
    is_edit: bool,
    reference: Option<(&str, &[u8])>,
) -> Result<ImageRequest, ApiFailure> {
    let size = if is_edit {
        "auto"
    } else {
        size_for_aspect_ratio(aspect_ratio)
    };
    let mut body = json!({
        "model": model,
        "prompt": prompt,
        "size": size,
        "quality": QUALITY,
        "n": 1,
        "output_format": "png",
    });
    let Some((media_type, bytes)) = reference else {
        return Ok(ImageRequest {
            path: "/images/generations",
            body,
        });
    };
    let image_url = format!(
        "data:{media_type};base64,{}",
        BASE64_STANDARD.encode(bytes)
    );
    if image_url.len() > MAX_IMAGE_URL_CHARS {
        return Err(ApiFailure::new(
            "ReferenceImageTooLarge",
            format!(
                "the OpenAI Image API accepts an input image of about 15 MB at most; this one is {} bytes",
                bytes.len()
            ),
        ));
    }
    body["images"] = json!([{ "image_url": image_url }]);
    Ok(ImageRequest {
        path: "/images/edits",
        body,
    })
}

/// Decodes the one image of a successful Image API response.
pub(crate) fn parse_image(body: &[u8]) -> Result<Vec<u8>, ApiFailure> {
    let missing =
        || ApiFailure::new("ImageOutputMissing", "the OpenAI Image API returned no image");
    let value = serde_json::from_slice::<Value>(body).map_err(|_| missing())?;
    let encoded = value
        .pointer("/data/0/b64_json")
        .and_then(Value::as_str)
        .ok_or_else(missing)?;
    BASE64_STANDARD.decode(encoded).map_err(|_| {
        ApiFailure::new(
            "ImageOutputMissing",
            "the OpenAI Image API returned an image that is not valid base64",
        )
    })
}

/// Turns a failed Image API response into the job's error. The upstream
/// message of an authentication failure can quote part of the key, so that
/// case uses fixed wording.
pub(crate) fn classify_error(
    status: u16,
    body: &[u8],
    retry_after: Option<Duration>,
) -> ApiFailure {
    let error = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value.get("error").cloned())
        .unwrap_or(Value::Null);
    let field = |name: &str| error.get(name).and_then(Value::as_str).unwrap_or_default();
    let (code, kind, upstream) = (field("code"), field("type"), field("message"));
    let detail = if upstream.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {}", upstream.chars().take(400).collect::<String>())
    };
    let mut failure = if status == 401 {
        ApiFailure::new(
            "OpenAiApiKeyRejected",
            "OpenAI rejected the API key (HTTP 401). Save a valid key in the Image Grid app's settings.",
        )
    } else if code == "model_not_found"
        || (upstream.contains("model") && (status == 404 || upstream.contains("does not exist")))
    {
        ApiFailure::new(
            "OpenAiImageModelUnavailable",
            format!(
                "OpenAI has no image model by this ID for this API key ({detail}). GPT Image 2.5 and 2.6 IDs carry a variant, such as gpt-image-2.6-sunburst or gpt-image-2.6-flare."
            ),
        )
    } else if code == "moderation_blocked" {
        ApiFailure::new(
            "ImageModerationBlocked",
            format!("OpenAI blocked this request in moderation ({detail})"),
        )
    } else if kind == "image_generation_user_error" {
        ApiFailure::new(
            "OpenAiImageRequestRejected",
            format!("OpenAI rejected this image request ({detail})"),
        )
    } else if kind == "insufficient_quota"
        || matches!(
            code,
            "insufficient_quota"
                | "credit_balance_exhausted"
                | "billing_hard_limit_reached"
                | "organization_spend_limit_exceeded"
                | "project_spend_limit_exceeded"
                | "organization_usage_limit_exceeded"
        )
    {
        ApiFailure::new(
            "OpenAiCreditExhausted",
            format!("the OpenAI API account has no usable credit or reached its limit ({detail})"),
        )
    } else if status == 429 {
        let mut failure = ApiFailure::new(
            "RateLimit",
            format!("the OpenAI Image API is rate-limited ({detail})"),
        );
        failure.retry_after = Some(retry_after.unwrap_or(RETRY_DELAY).min(RETRY_DELAY_MAX));
        failure
    } else if status >= 500 {
        let mut failure = ApiFailure::new(
            "OpenAiImageApiFailed",
            format!("the OpenAI Image API failed ({detail})"),
        );
        failure.retry_after = Some(RETRY_DELAY);
        failure
    } else {
        ApiFailure::new(
            "OpenAiImageApiFailed",
            format!("the OpenAI Image API refused the request ({detail})"),
        )
    };
    failure.status = Some(status);
    failure
}

/// Sends one Image API call and returns the image bytes.
pub(crate) async fn send(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    request: &ImageRequest,
) -> Result<Vec<u8>, ApiFailure> {
    let transport = |error: reqwest::Error| {
        // `without_url` keeps the message to the failure itself; the request
        // carries the key only in a header, which reqwest never prints.
        let mut failure = ApiFailure::new(
            "OpenAiImageApiFailed",
            format!("could not reach the OpenAI Image API: {}", error.without_url()),
        );
        failure.retry_after = Some(RETRY_DELAY);
        failure
    };
    let response = client
        .post(format!("{base_url}{}", request.path))
        .bearer_auth(api_key)
        .json(&request.body)
        .send()
        .await
        .map_err(transport)?;
    let status = response.status();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    let body = response.bytes().await.map_err(transport)?;
    if status.is_success() {
        parse_image(&body)
    } else {
        Err(classify_error(status.as_u16(), &body, retry_after))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paid_plan_draws_through_the_app_server_and_everything_else_through_the_api() {
        let chatgpt = |plan: &str| json!({"account": {"type": "chatgpt", "email": null, "planType": plan}});
        for plan in ["plus", "pro", "go", "team", "business", "enterprise", "edu"] {
            assert_eq!(route_for_account(&chatgpt(plan)), Ok(Route::AppServer), "{plan}");
        }
        assert_eq!(route_for_account(&chatgpt("free")), Ok(Route::Api));
        assert_eq!(route_for_account(&json!({"account": null})), Ok(Route::Api));
        assert_eq!(route_for_account(&json!({"requiresOpenaiAuth": true})), Ok(Route::Api));
        assert_eq!(
            route_for_account(&json!({"account": {"type": "apiKey"}})),
            Ok(Route::Api)
        );
        assert!(route_for_account(&chatgpt("unknown")).is_err());
        assert!(route_for_account(&json!({"account": {"type": "chatgpt"}})).is_err());
    }

    #[test]
    fn every_aspect_ratio_maps_to_a_size_the_image_api_accepts() {
        for (ratio, width, height) in [
            ("16:9", 16, 9),
            ("4:3", 4, 3),
            ("1:1", 1, 1),
            ("3:4", 3, 4),
            ("9:16", 9, 16),
        ] {
            let (w, h) = size_for_aspect_ratio(ratio).split_once('x').expect("WxH");
            let (w, h) = (w.parse::<u32>().unwrap(), h.parse::<u32>().unwrap());
            assert_eq!(w * height, h * width, "{ratio}");
            assert!(w % 16 == 0 && h % 16 == 0, "{ratio}");
            assert!((655_360..=8_294_400).contains(&(w * h)), "{ratio}");
        }
    }

    #[test]
    fn requests_take_the_generation_or_edit_endpoint_by_whether_an_image_is_attached() {
        let generate = build_request("gpt-image-2.6-sunburst", "a cat", "3:4", false, None).expect("generate");
        assert_eq!(generate.path, "/images/generations");
        assert_eq!(
            generate.body,
            json!({
                "model": "gpt-image-2.6-sunburst", "prompt": "a cat", "size": "1056x1408",
                "quality": "medium", "n": 1, "output_format": "png"
            })
        );

        let guided =
            build_request("gpt-image-2", "a cat", "1:1", false, Some(("image/png", b"abc"))).expect("guided");
        assert_eq!(guided.path, "/images/edits");
        assert_eq!(guided.body["model"], "gpt-image-2");
        assert_eq!(guided.body["size"], "1024x1024");
        assert_eq!(
            guided.body["images"],
            json!([{"image_url": "data:image/png;base64,YWJj"}])
        );

        let edit = build_request(DEFAULT_MODEL, "red hat", "16:9", true, Some(("image/jpeg", b"abc")))
            .expect("edit");
        assert_eq!(edit.path, "/images/edits");
        assert_eq!(edit.body["size"], "auto");

        let too_large = vec![0_u8; 16 * 1024 * 1024];
        let failure = build_request(DEFAULT_MODEL, "x", "1:1", true, Some(("image/png", &too_large)))
            .expect_err("over the input limit");
        assert_eq!(failure.code, "ReferenceImageTooLarge");
    }

    #[test]
    fn any_well_formed_model_id_is_accepted_and_malformed_ones_are_not() {
        for model in [
            "gpt-image-2.6-sunburst",
            "gpt-image-2.6-flare",
            "gpt-image-2.5-flare-2026-09-08",
            "gpt-image-2",
            "gpt-image-9.1-nova",
        ] {
            assert_eq!(validate_model(model), Ok(()), "{model}");
        }
        for model in ["", "gpt image 2", "gpt-image-2/../x", &"a".repeat(65)] {
            assert!(validate_model(model).is_err(), "{model:?}");
        }
    }

    #[test]
    fn a_successful_response_yields_the_image_bytes() {
        assert_eq!(
            parse_image(br#"{"data":[{"b64_json":"YWJj"}]}"#),
            Ok(b"abc".to_vec())
        );
        for body in [&b"{}"[..], br#"{"data":[]}"#, br#"{"data":[{"b64_json":"***"}]}"#, b"x"] {
            assert_eq!(parse_image(body).expect_err("no image").code, "ImageOutputMissing");
        }
    }

    #[test]
    fn failures_are_told_apart_and_only_transient_ones_are_retried() {
        let classify = |status, body: &str| classify_error(status, body.as_bytes(), None);

        let rejected = classify(
            401,
            r#"{"error":{"message":"Incorrect API key provided: sk-abc***xyz","code":"invalid_api_key"}}"#,
        );
        assert_eq!(rejected.code, "OpenAiApiKeyRejected");
        assert!(!rejected.message.contains("sk-"));
        assert_eq!(rejected.retry_after, None);

        let blocked = classify(
            400,
            r#"{"error":{"type":"image_generation_user_error","code":"moderation_blocked"}}"#,
        );
        assert_eq!(blocked.code, "ImageModerationBlocked");
        assert_eq!(blocked.retry_after, None);

        let credit = classify(429, r#"{"error":{"code":"credit_balance_exhausted"}}"#);
        assert_eq!(credit.code, "OpenAiCreditExhausted");
        assert_eq!(credit.retry_after, None);
        let quota = classify(429, r#"{"error":{"type":"insufficient_quota"}}"#);
        assert_eq!(quota.code, "OpenAiCreditExhausted");

        let limited = classify_error(
            429,
            br#"{"error":{"message":"Rate limit reached"}}"#,
            Some(Duration::from_secs(600)),
        );
        assert_eq!(limited.code, "RateLimit");
        assert_eq!(limited.status, Some(429));
        assert_eq!(limited.retry_after, Some(RETRY_DELAY_MAX));

        assert_eq!(classify(503, "upstream down").retry_after, Some(RETRY_DELAY));
        let unknown = classify(
            404,
            r#"{"error":{"message":"The model `gpt-image-2.6` does not exist","code":"model_not_found"}}"#,
        );
        assert_eq!(unknown.code, "OpenAiImageModelUnavailable");
        assert_eq!(unknown.retry_after, None);
        assert!(unknown.message.contains("gpt-image-2.6-sunburst"));
        // What the API returned for `gpt-image-2.6` on 2026-11-24.
        let bare = classify(
            400,
            r#"{"error":{"message":"The model 'gpt-image-2.6' does not exist.","type":"image_generation_user_error"}}"#,
        );
        assert_eq!(bare.code, "OpenAiImageModelUnavailable");

        let refused = classify(400, r#"{"error":{"message":"bad size"}}"#);
        assert_eq!(refused.code, "OpenAiImageApiFailed");
        assert_eq!(refused.retry_after, None);
        assert!(refused.message.contains("bad size"));
    }
}
