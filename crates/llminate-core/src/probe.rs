//! Endpoint probe for onboarding: inspect an OpenAI-compatible server.

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
pub struct ProbeReport {
    pub probe_version: u32,
    pub ok: bool,
    pub target: Option<String>,
    pub url: String,
    pub auth: AuthInfo,
    pub engine: EngineInfo,
    pub models: Vec<ModelInfo>,
    pub latency_ms: u64,
    pub health: HealthInfo,
    pub endpoints: BTreeMap<String, EndpointInfo>,
    pub errors: Vec<ProbeError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthInfo { pub status: String, pub http_status: Option<u16> }

#[derive(Debug, Clone, Serialize)]
pub struct EngineInfo { pub kind: String, pub version: Option<String> }

#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo { pub id: String, pub owned_by: Option<String> }

#[derive(Debug, Clone, Serialize)]
pub struct HealthInfo { pub status: String, pub latency_ms: Option<u64> }

#[derive(Debug, Clone, Serialize)]
pub struct EndpointInfo { pub status: String, pub http_status: Option<u16> }

#[derive(Debug, Clone, Serialize)]
pub struct ProbeError { pub probe: String, pub message: String }

pub fn endpoint_status(code: u16) -> &'static str {
    match code {
        200..=299 => "ok",
        401 | 403 => "denied",
        404 | 405 => "missing",
        _ => "error",
    }
}

pub fn auth_status(has_key: bool, ok_2xx: bool, http_status: Option<u16>) -> &'static str {
    match (ok_2xx, http_status, has_key) {
        (true, _, true) => "ok",
        (true, _, false) => "not_required",
        (false, Some(401 | 403), true) => "rejected",
        (false, Some(401 | 403), false) => "required_missing",
        (false, None, _) => "unreachable",
        _ => "error",
    }
}

pub fn parse_models(body: &str) -> Vec<ModelInfo> {
    let v: Value = match serde_json::from_str(body) { Ok(v) => v, Err(_) => return Vec::new() };
    v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            m["id"].as_str().map(|id| ModelInfo {
                id: id.to_string(),
                owned_by: m["owned_by"].as_str().map(str::to_string),
            })
        })
        .collect()
}

pub fn classify_engine(version_body: Option<&str>, model_info_body: Option<&str>) -> EngineInfo {
    if let Some(b) = version_body {
        if let Ok(v) = serde_json::from_str::<Value>(b) {
            if let Some(ver) = v["version"].as_str() {
                return EngineInfo { kind: "vllm".into(), version: Some(ver.to_string()) };
            }
        }
    }
    if let Some(b) = model_info_body {
        if let Ok(v) = serde_json::from_str::<Value>(b) {
            if let Some(p) = v["model_path"].as_str() {
                return EngineInfo { kind: "sglang".into(), version: Some(p.to_string()) };
            }
        }
    }
    EngineInfo { kind: "openai-compatible".into(), version: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_status_map() {
        assert_eq!(endpoint_status(200), "ok");
        assert_eq!(endpoint_status(404), "missing");
        assert_eq!(endpoint_status(405), "missing");
        assert_eq!(endpoint_status(401), "denied");
        assert_eq!(endpoint_status(500), "error");
    }

    #[test]
    fn auth_matrix() {
        assert_eq!(auth_status(true, true, Some(200)), "ok");
        assert_eq!(auth_status(false, true, Some(200)), "not_required");
        assert_eq!(auth_status(true, false, Some(401)), "rejected");
        assert_eq!(auth_status(false, false, Some(401)), "required_missing");
        assert_eq!(auth_status(true, false, None), "unreachable");
    }

    #[test]
    fn models_parsed() {
        let b = r#"{"object":"list","data":[{"id":"m1","owned_by":"vllm"},{"id":"m2"}]}"#;
        let m = parse_models(b);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].id, "m1");
        assert_eq!(m[0].owned_by.as_deref(), Some("vllm"));
        assert_eq!(m[1].owned_by, None);
        assert!(parse_models("not json").is_empty());
    }

    #[test]
    fn engine_classification() {
        assert_eq!(classify_engine(Some(r#"{"version":"0.32"}"#), None).kind, "vllm");
        assert_eq!(classify_engine(None, Some(r#"{"model_path":"x"}"#)).kind, "sglang");
        assert_eq!(classify_engine(None, None).kind, "openai-compatible");
    }
}
