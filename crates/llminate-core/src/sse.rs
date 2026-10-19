//! SSE parsing for OpenAI-compatible streaming responses.

use serde_json::Value;

/// Extract assistant text from a non-streaming chat completion body.
pub fn content_from_message(v: &Value) -> Option<String> {
    nonempty(&v["choices"][0]["message"]["content"])
        .or_else(|| nonempty(&v["choices"][0]["message"]["reasoning"]))
}

/// Extract assistant text from a single streaming `delta` object.
pub fn content_from_delta(v: &Value) -> Option<String> {
    let delta = &v["choices"][0]["delta"];
    nonempty(&delta["content"]).or_else(|| nonempty(&delta["reasoning"]))
}

/// Parse one SSE line (`data: {...}`) and return the delta text, if any.
pub fn content_from_line(line: &str) -> Option<String> {
    let data = line.strip_prefix("data:")?.trim_start();
    if data.is_empty() {
        return None;
    }
    if data.starts_with('[') {
        return None;
    }
    let v: Value = serde_json::from_str(data).ok()?;
    content_from_delta(&v)
}
fn nonempty(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(|s| s.to_string())
}
