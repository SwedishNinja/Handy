use crate::settings::{AuthMethod, PostProcessProvider};
use log::debug;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE, REFERER, USER_AGENT};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct JsonSchema {
    name: String,
    strict: bool,
    schema: Value,
}

#[derive(Debug, Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    format_type: String,
    json_schema: JsonSchema,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct ReasoningConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude: Option<bool>,
}

/// Default upper bound on response tokens. Generous enough for transcript-
/// cleaning prompts on long recordings, small enough that gateways which bill
/// per-output-token don't surprise the user. Anthropic-backed gateways require
/// this field; OpenAI-compatible ones treat it as a soft cap, so sending it
/// universally is safe.
const DEFAULT_MAX_TOKENS: u32 = 4096;

#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    /// Required by Anthropic and Anthropic-proxying gateways. Sent on every
    /// request.
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningConfig>,
}

/// Extract the assistant's text from an LLM completion response.
///
/// Handles both OpenAI-style (`choices[0].message.content`) and Anthropic-
/// style (`content[0].text`) shapes, since custom gateways often translate
/// requests but pass through the upstream's native response format.
fn extract_completion_content(body: &Value) -> Option<String> {
    // OpenAI / OpenAI-compatible: { "choices": [{ "message": { "content": "..." } }] }
    if let Some(content) = body
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
    {
        if !content.is_empty() {
            return Some(content.to_string());
        }
    }

    // Anthropic native: { "content": [{ "type": "text", "text": "..." }] }
    if let Some(blocks) = body.get("content").and_then(|c| c.as_array()) {
        for block in blocks {
            let is_text = block
                .get("type")
                .and_then(|t| t.as_str())
                .map_or(true, |t| t == "text");
            if is_text {
                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                    if !text.is_empty() {
                        return Some(text.to_string());
                    }
                }
            }
        }
    }

    None
}

/// Turn an HTTP error from an LLM provider into a safe diagnostic string.
///
/// **Never echoes the raw response body.** A self-hosted / custom provider
/// could echo request headers (including the `Authorization: Bearer <key>`
/// header) in its error response, and the caller pipes our return value into
/// `error!` logs. To stay safe regardless of the provider's behavior, we:
///
/// 1. Try to parse the body as JSON and extract `error.message` (the OpenAI /
///    Anthropic / Groq / OpenRouter convention). That field is provider-
///    authored and shouldn't contain echoed request headers.
/// 2. Fall back to `(status N, body suppressed)` if the body isn't JSON in
///    that shape.
fn sanitize_llm_error(status: reqwest::StatusCode, body: &str) -> String {
    let extracted = serde_json::from_str::<Value>(body).ok().and_then(|v| {
        v.get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .map(|s| s.to_string())
    });
    match extracted {
        Some(msg) => format!("API request failed (status {status}): {msg}"),
        None => format!("API request failed (status {status}); body suppressed"),
    }
}

/// Build headers for API requests.
///
/// The auth header is driven by the provider's `auth_method` field
/// (configurable in the UI for the custom provider; vendor-fixed for
/// built-ins). Anthropic additionally requires `anthropic-version`.
fn build_headers(provider: &PostProcessProvider, api_key: &str) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();

    // Common headers
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        REFERER,
        HeaderValue::from_static("https://github.com/cjpais/Handy"),
    );
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static("Handy/1.0 (+https://github.com/cjpais/Handy)"),
    );
    headers.insert("X-Title", HeaderValue::from_static("Handy"));

    if provider.id == "anthropic" {
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    }

    if !api_key.is_empty() {
        match provider.auth_method {
            AuthMethod::BearerToken => {
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_str(&format!("Bearer {}", api_key))
                        .map_err(|e| format!("Invalid authorization header value: {}", e))?,
                );
            }
            AuthMethod::XApiKey => {
                headers.insert(
                    "x-api-key",
                    HeaderValue::from_str(api_key)
                        .map_err(|e| format!("Invalid API key header value: {}", e))?,
                );
            }
        }
    }

    Ok(headers)
}

/// Create an HTTP client with provider-specific headers
fn create_client(provider: &PostProcessProvider, api_key: &str) -> Result<reqwest::Client, String> {
    let headers = build_headers(provider, api_key)?;
    reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))
}

/// Send a chat completion request to an OpenAI-compatible API
/// Returns Ok(Some(content)) on success, Ok(None) if response has no content,
/// or Err on actual errors (HTTP, parsing, etc.)
pub async fn send_chat_completion(
    provider: &PostProcessProvider,
    api_key: String,
    model: &str,
    prompt: String,
    reasoning_effort: Option<String>,
    reasoning: Option<ReasoningConfig>,
) -> Result<Option<String>, String> {
    send_chat_completion_with_schema(
        provider,
        api_key,
        model,
        prompt,
        None,
        None,
        reasoning_effort,
        reasoning,
    )
    .await
}

/// Send a chat completion request with structured output support
/// When json_schema is provided, uses structured outputs mode
/// system_prompt is used as the system message when provided
/// reasoning_effort sets the OpenAI-style top-level field (e.g., "none", "low", "medium", "high")
/// reasoning sets the OpenRouter-style nested object (effort + exclude)
pub async fn send_chat_completion_with_schema(
    provider: &PostProcessProvider,
    api_key: String,
    model: &str,
    user_content: String,
    system_prompt: Option<String>,
    json_schema: Option<Value>,
    reasoning_effort: Option<String>,
    reasoning: Option<ReasoningConfig>,
) -> Result<Option<String>, String> {
    let base_url = provider.base_url.trim_end_matches('/');
    let url = format!("{}/chat/completions", base_url);

    debug!("Sending chat completion request to: {}", url);

    let client = create_client(provider, &api_key)?;

    // Build messages vector
    let mut messages = Vec::new();

    // Add system prompt if provided
    if let Some(system) = system_prompt {
        messages.push(ChatMessage {
            role: "system".to_string(),
            content: system,
        });
    }

    // Add user message
    messages.push(ChatMessage {
        role: "user".to_string(),
        content: user_content,
    });

    // Build response_format if schema is provided
    let response_format = json_schema.map(|schema| ResponseFormat {
        format_type: "json_schema".to_string(),
        json_schema: JsonSchema {
            name: "transcription_output".to_string(),
            strict: true,
            schema,
        },
    });

    let request_body = ChatCompletionRequest {
        model: model.to_string(),
        messages,
        max_tokens: DEFAULT_MAX_TOKENS,
        response_format,
        reasoning_effort,
        reasoning,
    };

    let response = client
        .post(&url)
        .json(&request_body)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(sanitize_llm_error(status, &body));
    }

    let body: Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse API response: {}", e))?;

    Ok(extract_completion_content(&body))
}

/// Fetch available models from an OpenAI-compatible API
/// Returns a list of model IDs
pub async fn fetch_models(
    provider: &PostProcessProvider,
    api_key: String,
) -> Result<Vec<String>, String> {
    let base_url = provider.base_url.trim_end_matches('/');
    let url = format!("{}/models", base_url);

    debug!("Fetching models from: {}", url);

    let client = create_client(provider, &api_key)?;

    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Failed to fetch models: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(sanitize_llm_error(status, &body));
    }

    let parsed: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    let mut models = Vec::new();

    // Handle OpenAI format: { data: [ { id: "..." }, ... ] }
    if let Some(data) = parsed.get("data").and_then(|d| d.as_array()) {
        for entry in data {
            if let Some(id) = entry.get("id").and_then(|i| i.as_str()) {
                models.push(id.to_string());
            } else if let Some(name) = entry.get("name").and_then(|n| n.as_str()) {
                models.push(name.to_string());
            }
        }
    }
    // Handle array format: [ "model1", "model2", ... ]
    else if let Some(array) = parsed.as_array() {
        for entry in array {
            if let Some(model) = entry.as_str() {
                models.push(model.to_string());
            }
        }
    }

    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(code: u16) -> reqwest::StatusCode {
        reqwest::StatusCode::from_u16(code).unwrap()
    }

    fn provider(id: &str, auth: AuthMethod) -> PostProcessProvider {
        PostProcessProvider {
            id: id.to_string(),
            label: id.to_string(),
            base_url: "https://example.test".to_string(),
            allow_base_url_edit: false,
            models_endpoint: Some("/models".to_string()),
            supports_structured_output: false,
            auth_method: auth,
        }
    }

    // ---- build_headers ------------------------------------------------------

    #[test]
    fn bearer_auth_sends_authorization_bearer_header() {
        let p = provider("openai", AuthMethod::BearerToken);
        let headers = build_headers(&p, "sk-test-key").unwrap();
        assert_eq!(headers.get(AUTHORIZATION).unwrap(), "Bearer sk-test-key");
        assert!(headers.get("x-api-key").is_none());
    }

    #[test]
    fn x_api_key_auth_sends_x_api_key_header() {
        let p = provider("custom", AuthMethod::XApiKey);
        let headers = build_headers(&p, "raw-key-no-prefix").unwrap();
        assert_eq!(headers.get("x-api-key").unwrap(), "raw-key-no-prefix");
        assert!(
            headers.get(AUTHORIZATION).is_none(),
            "must not also send Bearer when using x-api-key"
        );
    }

    #[test]
    fn anthropic_provider_includes_version_header() {
        let p = provider("anthropic", AuthMethod::XApiKey);
        let headers = build_headers(&p, "sk-ant-key").unwrap();
        assert_eq!(headers.get("anthropic-version").unwrap(), "2023-06-01");
        assert_eq!(headers.get("x-api-key").unwrap(), "sk-ant-key");
    }

    #[test]
    fn empty_api_key_skips_auth_header_entirely() {
        let p = provider("openai", AuthMethod::BearerToken);
        let headers = build_headers(&p, "").unwrap();
        assert!(headers.get(AUTHORIZATION).is_none());
        assert!(headers.get("x-api-key").is_none());
    }

    // ---- extract_completion_content -----------------------------------------

    #[test]
    fn extract_handles_openai_shape() {
        let body = serde_json::json!({
            "id": "chatcmpl-xxx",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "Hello, world." },
                "finish_reason": "stop"
            }]
        });
        assert_eq!(
            extract_completion_content(&body),
            Some("Hello, world.".to_string())
        );
    }

    #[test]
    fn extract_handles_anthropic_shape() {
        let body = serde_json::json!({
            "id": "msg_xxx",
            "type": "message",
            "role": "assistant",
            "content": [
                { "type": "text", "text": "Hello from Claude." }
            ],
            "stop_reason": "end_turn"
        });
        assert_eq!(
            extract_completion_content(&body),
            Some("Hello from Claude.".to_string())
        );
    }

    #[test]
    fn extract_skips_non_text_blocks_in_anthropic_shape() {
        // Claude can return interleaved "thinking" and "text" blocks. We only
        // want the user-facing text — internal reasoning must be skipped.
        let body = serde_json::json!({
            "content": [
                { "type": "thinking", "text": "internal..." },
                { "type": "text", "text": "Final answer." }
            ]
        });
        assert_eq!(
            extract_completion_content(&body),
            Some("Final answer.".to_string())
        );
    }

    #[test]
    fn extract_returns_none_for_empty_choices() {
        let body = serde_json::json!({ "choices": [] });
        assert_eq!(extract_completion_content(&body), None);
    }

    #[test]
    fn extract_returns_none_when_content_string_is_empty() {
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "" } }]
        });
        assert_eq!(extract_completion_content(&body), None);
    }

    #[test]
    fn extract_returns_none_for_unknown_shape() {
        let body = serde_json::json!({ "result": "Hello" });
        assert_eq!(extract_completion_content(&body), None);
    }

    #[test]
    fn extract_prefers_openai_when_both_shapes_present() {
        // Defensive: if a gateway forwards both, OpenAI shape was the explicit
        // request; honor that.
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "from-openai-path" } }],
            "content": [{ "type": "text", "text": "from-anthropic-path" }]
        });
        assert_eq!(
            extract_completion_content(&body),
            Some("from-openai-path".to_string())
        );
    }

    // ---- sanitize_llm_error -------------------------------------------------

    #[test]
    fn sanitize_extracts_openai_style_error_message() {
        let body = r#"{"error":{"message":"Invalid model 'foo'","type":"invalid_request_error"}}"#;
        let out = sanitize_llm_error(status(400), body);
        assert!(out.contains("Invalid model 'foo'"));
        assert!(out.contains("400"));
    }

    #[test]
    fn sanitize_suppresses_body_when_not_json() {
        // Use a body string that doesn't collide with the HTTP status's
        // reason phrase (e.g. don't put "Bad Gateway" in a 502 body).
        let body = "<html><body>nginx upstream error xyzzy</body></html>";
        let out = sanitize_llm_error(status(502), body);
        assert!(
            !out.contains("xyzzy"),
            "raw body must not be echoed; got: {out}"
        );
        assert!(out.contains("body suppressed"));
        assert!(out.contains("502"));
    }

    #[test]
    fn sanitize_suppresses_body_with_unknown_json_shape() {
        let body = r#"{"detail": "Internal server error"}"#;
        let out = sanitize_llm_error(status(500), body);
        assert!(!out.contains("Internal server error"));
        assert!(out.contains("body suppressed"));
    }

    #[test]
    fn sanitize_does_not_leak_echoed_authorization_header() {
        // Worst-case: a misbehaving custom gateway echoes the request.
        // `error.message` extraction sidesteps this — we never see auth headers
        // in that field. But if the body shape isn't recognized, we fall back
        // to "body suppressed" rather than echo the raw body.
        let leaky_body = r#"{"received_headers":{"authorization":"Bearer sk-very-secret"}}"#;
        let out = sanitize_llm_error(status(400), leaky_body);
        assert!(
            !out.contains("sk-very-secret"),
            "API key must not leak even if provider echoes headers"
        );
        assert!(!out.contains("Bearer"));
    }

    #[test]
    fn sanitize_does_not_leak_secret_when_extracted_message_omits_it() {
        // The error.message field is provider-authored and shouldn't echo
        // headers — but verify the extractor doesn't accidentally pull
        // siblings like `received_headers` along for the ride.
        let body = r#"{
            "error": {"message": "Invalid request"},
            "received_headers": {"authorization": "Bearer sk-very-secret"}
        }"#;
        let out = sanitize_llm_error(status(400), body);
        assert!(out.contains("Invalid request"));
        assert!(!out.contains("sk-very-secret"));
    }

    #[test]
    fn sanitize_handles_empty_body() {
        let out = sanitize_llm_error(status(401), "");
        assert!(out.contains("401"));
        assert!(out.contains("body suppressed"));
    }
}
