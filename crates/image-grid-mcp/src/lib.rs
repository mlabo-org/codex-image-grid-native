use image_grid_core::{
    MAX_PROMPTS, MAX_RUN_JOBS, MAX_VARIANTS_PER_PROMPT, MAX_WAIT_MS, validate_reference_image,
};
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use std::path::Path;

pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
pub const TOOL_NAME: &str = "generate_image_grid";

const TOOL_DESCRIPTION: &str = "Generate project-ready image variants from Prompt Batch input. \
Auto-launches the local Image Grid app or web server when possible, then returns handoff.md, \
absolute output paths, display-safe image URLs, and Codex Markdown.";
const SERVER_INSTRUCTIONS: &str = "Use generate_image_grid when the user needs project-specific \
thumbnails, visual variants, or Prompt Batch image generation. Return and reuse handoff.md, \
absolute output paths, imageUrls, and codexMarkdown.";
const GENERATION_UNAVAILABLE: &str =
    "native image generation is not implemented in this runnable slice";

pub fn serve<R: BufRead, W: Write>(reader: R, mut writer: W) -> io::Result<()> {
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle_input(&message),
            Err(error) => Some(rpc_error(
                Value::Null,
                -32700,
                format!("parse error: {error}"),
            )),
        };
        if let Some(response) = response {
            serde_json::to_writer(&mut writer, &response).map_err(io::Error::other)?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }
    }
    Ok(())
}

pub fn handle_input(message: &Value) -> Option<Value> {
    if let Some(batch) = message.as_array() {
        let responses: Vec<Value> = batch.iter().filter_map(handle_request).collect();
        return (!responses.is_empty()).then_some(Value::Array(responses));
    }
    handle_request(message)
}

fn handle_request(message: &Value) -> Option<Value> {
    let id = message.get("id")?.clone();
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();

    match method {
        "initialize" => Some(rpc_result(id, initialize_result(message.get("params")))),
        "ping" => Some(rpc_result(id, json!({}))),
        "tools/list" => Some(rpc_result(id, json!({ "tools": [tool_record()] }))),
        "tools/call" => Some(handle_tool_call(id, message.get("params"))),
        _ => Some(rpc_error(id, -32601, format!("unknown method: {method}"))),
    }
}

fn initialize_result(params: Option<&Value>) -> Value {
    let requested_protocol = params
        .and_then(|value| value.get("protocolVersion"))
        .filter(|value| javascript_truthy(value))
        .cloned()
        .unwrap_or_else(|| Value::String(MCP_PROTOCOL_VERSION.to_owned()));

    json!({
        "protocolVersion": requested_protocol,
        "capabilities": {
            "tools": {
                "listChanged": false
            }
        },
        "serverInfo": {
            "name": "codex-image-grid-native",
            "title": "Codex Image Grid Native",
            "version": env!("CARGO_PKG_VERSION")
        },
        "instructions": SERVER_INSTRUCTIONS
    })
}

fn handle_tool_call(id: Value, params: Option<&Value>) -> Value {
    let name = params
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if name != TOOL_NAME {
        let displayed_name = if name.is_empty() { "(missing)" } else { name };
        return rpc_error(
            id,
            -32602,
            format!("unknown tool: {displayed_name}"),
        );
    }

    let arguments = params
        .and_then(|value| value.get("arguments"))
        .unwrap_or(&Value::Null);
    let result = match validate_tool_arguments(arguments) {
        Ok(()) => tool_error(GENERATION_UNAVAILABLE),
        Err(message) => tool_error(message),
    };
    rpc_result(id, result)
}

fn validate_tool_arguments(arguments: &Value) -> Result<(), String> {
    let prompts = arguments
        .get("prompts")
        .and_then(Value::as_array)
        .ok_or_else(|| "prompts must be an array".to_owned())?;
    if prompts.is_empty() {
        return Err("prompts array must contain at least one prompt".to_owned());
    }
    if prompts.len() > MAX_PROMPTS {
        return Err(format!(
            "prompt batch is limited to {MAX_PROMPTS} prompts"
        ));
    }
    for (index, prompt) in prompts.iter().enumerate() {
        let prompt = prompt
            .as_str()
            .ok_or_else(|| format!("prompt {} must be a string", index + 1))?;
        if prompt.trim().is_empty() {
            return Err(format!("prompt {} must not be empty", index + 1));
        }
    }

    let count = match arguments.get("count") {
        None => 1_i128,
        Some(value) => json_integer(value)
            .ok_or_else(|| "count must be an integer".to_owned())?,
    };
    if !(1..=i128::from(MAX_VARIANTS_PER_PROMPT)).contains(&count) {
        return Err(format!(
            "count must be between 1 and {MAX_VARIANTS_PER_PROMPT}"
        ));
    }
    if prompts.len() * count as usize > MAX_RUN_JOBS {
        return Err(format!("a run is limited to {MAX_RUN_JOBS} total jobs"));
    }

    validate_enum(
        arguments,
        "mood",
        &[
            "warm-mascot",
            "clean-thumbnail",
            "editorial-soft",
            "cinematic",
            "minimal-product",
        ],
    )?;
    validate_enum(arguments, "engine", &["app-server-image", "codex-svg"])?;
    validate_enum(
        arguments,
        "aspectRatio",
        &["16:9", "4:3", "1:1", "3:4", "9:16"],
    )?;

    if let Some(wait_ms) = arguments.get("waitMs") {
        let valid = json_integer(wait_ms)
            .is_some_and(|value| (0..=i128::from(MAX_WAIT_MS)).contains(&value));
        if !valid {
            return Err(format!(
                "waitMs must be an integer between 0 and {MAX_WAIT_MS}"
            ));
        }
    }
    if arguments
        .get("referencePremise")
        .is_some_and(|value| !value.is_string())
    {
        return Err("referencePremise must be a string".to_owned());
    }

    if let Some(reference_path) = arguments.get("referenceImagePath") {
        let reference_path = reference_path
            .as_str()
            .ok_or_else(|| "referenceImagePath must be a string".to_owned())?;
        if !reference_path.is_empty() {
            validate_reference_image(Path::new(reference_path))
                .map_err(|error| error.to_string())?;
        }
    }

    Ok(())
}

fn validate_enum(arguments: &Value, name: &str, accepted: &[&str]) -> Result<(), String> {
    let Some(value) = arguments.get(name) else {
        return Ok(());
    };
    let value = value.as_str().ok_or_else(|| {
        format!(
            "{name} must be one of: {}",
            accepted.join(", ")
        )
    })?;
    if !accepted.contains(&value) {
        return Err(format!(
            "{name} must be one of: {}",
            accepted.join(", ")
        ));
    }
    Ok(())
}

pub fn tool_record() -> Value {
    json!({
        "name": TOOL_NAME,
        "title": "Generate Image Grid",
        "description": TOOL_DESCRIPTION,
        "inputSchema": {
            "type": "object",
            "description": "Prompt count multiplied by variants per prompt must not exceed 24 total jobs.",
            "properties": {
                "prompts": {
                    "type": "array",
                    "description": "Prompt Batch input. Pass project-specific visual directions.",
                    "minItems": 1,
                    "maxItems": 12,
                    "items": {
                        "type": "string",
                        "minLength": 1,
                        "pattern": "\\S"
                    }
                },
                "count": {
                    "type": "integer",
                    "description": "Variants per prompt. prompts.length × count must be at most 24.",
                    "minimum": 1,
                    "maximum": 6,
                    "default": 1
                },
                "mood": {
                    "type": "string",
                    "enum": [
                        "warm-mascot",
                        "clean-thumbnail",
                        "editorial-soft",
                        "cinematic",
                        "minimal-product"
                    ],
                    "default": "warm-mascot"
                },
                "engine": {
                    "type": "string",
                    "enum": ["app-server-image", "codex-svg"],
                    "default": "app-server-image"
                },
                "aspectRatio": {
                    "type": "string",
                    "enum": ["16:9", "4:3", "1:1", "3:4", "9:16"],
                    "default": "16:9"
                },
                "referencePremise": {
                    "type": "string",
                    "description": "Optional visual identity notes from the current product or reference image."
                },
                "referenceImagePath": {
                    "type": "string",
                    "description": "Optional absolute local PNG, JPEG, or WebP path to attach as the visual reference."
                },
                "waitMs": {
                    "type": "integer",
                    "description": "Optional short wait for completion before returning.",
                    "minimum": 0,
                    "maximum": 120000,
                    "default": 0
                }
            },
            "required": ["prompts"],
            "allOf": [
                conditional_prompt_limit(1, 12),
                conditional_prompt_limit(2, 12),
                conditional_prompt_limit(3, 8),
                conditional_prompt_limit(4, 6),
                conditional_prompt_limit(5, 4),
                conditional_prompt_limit(6, 4)
            ],
            "x-image-grid-total-job-constraint": {
                "formula": "prompts.length * count",
                "maximum": 24
            }
        },
        "annotations": {
            "title": "Generate Image Grid",
            "readOnlyHint": false,
            "destructiveHint": false,
            "idempotentHint": false,
            "openWorldHint": false
        }
    })
}

fn conditional_prompt_limit(count: u8, max_items: u8) -> Value {
    json!({
        "if": {
            "required": ["count"],
            "properties": {
                "count": {
                    "const": count
                }
            }
        },
        "then": {
            "properties": {
                "prompts": {
                    "maxItems": max_items
                }
            }
        }
    })
}

fn tool_error(message: impl Into<String>) -> Value {
    json!({
        "content": [
            {
                "type": "text",
                "text": message.into()
            }
        ],
        "isError": true
    })
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn rpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message.into()
        }
    })
}

fn javascript_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|number| number != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn json_integer(value: &Value) -> Option<i128> {
    if let Some(value) = value.as_i64() {
        return Some(i128::from(value));
    }
    if let Some(value) = value.as_u64() {
        return Some(i128::from(value));
    }
    let value = value.as_f64()?;
    if value.is_finite() && value.fract() == 0.0 {
        Some(value as i128)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn provider_free_transcript_covers_initialize_list_and_call() {
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":",
            "{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{},",
            "\"clientInfo\":{\"name\":\"native-smoke\",\"version\":\"0.1.0\"}}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\",\"params\":{}}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":",
            "{\"name\":\"generate_image_grid\",\"arguments\":{\"prompts\":[]}}}\n"
        );
        let mut output = Vec::new();

        serve(Cursor::new(input), &mut output).expect("MCP transcript");

        let responses: Vec<Value> = String::from_utf8(output)
            .expect("UTF-8 output")
            .lines()
            .map(|line| serde_json::from_str(line).expect("JSON response"))
            .collect();
        assert_eq!(responses.len(), 3);
        assert_eq!(responses[0]["id"], 1);
        assert_eq!(
            responses[0]["result"]["protocolVersion"],
            MCP_PROTOCOL_VERSION
        );
        assert_eq!(
            responses[0]["result"]["serverInfo"]["name"],
            "codex-image-grid-native"
        );
        assert_eq!(responses[1]["id"], 2);
        assert_eq!(
            responses[1]["result"]["tools"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(
            responses[1]["result"]["tools"][0]["name"],
            "generate_image_grid"
        );
        assert_eq!(
            responses[2],
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "result": {
                    "content": [{
                        "type": "text",
                        "text": "prompts array must contain at least one prompt"
                    }],
                    "isError": true
                }
            })
        );
    }

    #[test]
    fn tool_schema_preserves_the_frozen_public_limits() {
        let tool = tool_record();
        assert_eq!(tool["inputSchema"]["required"], json!(["prompts"]));
        assert_eq!(
            tool["inputSchema"]["properties"]["prompts"]["maxItems"],
            MAX_PROMPTS
        );
        assert_eq!(
            tool["inputSchema"]["properties"]["count"]["maximum"],
            MAX_VARIANTS_PER_PROMPT
        );
        assert_eq!(
            tool["inputSchema"]["x-image-grid-total-job-constraint"]["maximum"],
            MAX_RUN_JOBS
        );
        assert_eq!(
            tool["inputSchema"]["properties"]["waitMs"]["maximum"],
            MAX_WAIT_MS
        );
        assert!(tool["inputSchema"].get("additionalProperties").is_none());
    }

    #[test]
    fn strict_argument_errors_match_the_frozen_messages() {
        assert_eq!(
            validate_tool_arguments(&json!({"prompts": ["valid"], "count": 1.5})),
            Err("count must be an integer".to_owned())
        );
        assert_eq!(
            validate_tool_arguments(&json!({"prompts": ["valid"], "waitMs": -1})),
            Err("waitMs must be an integer between 0 and 120000".to_owned())
        );
        assert_eq!(
            validate_tool_arguments(&json!({
                "prompts": ["valid"],
                "referenceImagePath": "relative.png"
            })),
            Err("referenceImagePath must be an absolute local file path".to_owned())
        );
    }
}
