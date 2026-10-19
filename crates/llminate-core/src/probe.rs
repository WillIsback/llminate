//! Endpoint probe for onboarding: inspect an OpenAI-compatible server.

use crate::client::Client;
use crate::error::Error;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Instant;

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
pub struct AuthInfo {
    pub status: String,
    pub http_status: Option<u16>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineInfo {
    pub kind: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub owned_by: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthInfo {
    pub status: String,
    pub latency_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EndpointInfo {
    pub status: String,
    pub http_status: Option<u16>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeError {
    pub probe: String,
    pub message: String,
}

pub fn endpoint_status(code: u16) -> &'static str {
    match code {
        200..=299 => "ok",
        401 | 403 => "denied",
        404 | 405 => "missing",
        _ => "error",
    }
}

pub fn auth_status(has_key: bool, http_status: Option<u16>) -> &'static str {
    match (http_status, has_key) {
        (Some(c), _) if (200..=299).contains(&c) => {
            if has_key {
                "ok"
            } else {
                "not_required"
            }
        }
        (Some(401 | 403), true) => "rejected",
        (Some(401 | 403), false) => "required_missing",
        (None, _) => "unreachable",
        _ => "error",
    }
}

pub fn parse_models(body: &str) -> Vec<ModelInfo> {
    let v: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
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
                return EngineInfo {
                    kind: "vllm".into(),
                    version: Some(ver.to_string()),
                };
            }
        }
    }
    if let Some(b) = model_info_body {
        if let Ok(v) = serde_json::from_str::<Value>(b) {
            if let Some(p) = v["model_path"].as_str() {
                return EngineInfo {
                    kind: "sglang".into(),
                    version: Some(p.to_string()),
                };
            }
        }
    }
    EngineInfo {
        kind: "openai-compatible".into(),
        version: None,
    }
}

/// Engine kind for the report: true "unknown" is only for endpoints that did
/// not answer in an OpenAI-compatible way. Also note: a 200 with an unparseable
/// body yields ok=false but stays "openai-compatible" (not "unknown").
pub fn engine_kind(models_ok: bool, classified_kind: &str) -> String {
    if !models_ok && classified_kind == "openai-compatible" {
        "unknown".to_string()
    } else {
        classified_kind.to_string()
    }
}

/// Overall verdict: auth OK and at least one model.
pub fn probe_ok(auth_status: &str, models_len: usize) -> bool {
    matches!(auth_status, "ok" | "not_required") && models_len > 0
}

/// Probe one endpoint and build its report. Never panics; optional probes are
/// best-effort and recorded in `endpoints`. `errors` only lists the primary
/// `/v1/models` failure and optional endpoints that are `denied`, `unreachable`
/// or `error` — a benign `missing` optional endpoint is not an error.
pub async fn probe(client: &Client, target: Option<String>) -> ProbeReport {
    let has_key = client.has_api_key();
    let url = client.base_url().to_string();
    let mut endpoints = BTreeMap::new();

    // /v1/models — primary signal
    let t0 = Instant::now();
    let models_res = client.get("/v1/models").await;
    let latency_ms = t0.elapsed().as_millis() as u64;

    let (auth, models, models_ep) = match &models_res {
        Ok(body) => (
            AuthInfo {
                status: auth_status(has_key, Some(200)).into(),
                http_status: Some(200),
            },
            parse_models(body),
            EndpointInfo {
                status: "ok".into(),
                http_status: Some(200),
            },
        ),
        Err(Error::Http { status, .. }) => (
            AuthInfo {
                status: auth_status(has_key, Some(*status)).into(),
                http_status: Some(*status),
            },
            Vec::new(),
            EndpointInfo {
                status: endpoint_status(*status).into(),
                http_status: Some(*status),
            },
        ),
        Err(_) => (
            AuthInfo {
                status: auth_status(has_key, None).into(),
                http_status: None,
            },
            Vec::new(),
            EndpointInfo {
                status: "unreachable".into(),
                http_status: None,
            },
        ),
    };
    let models_ok = models_ep.status == "ok";
    let primary_unreachable = models_ep.status == "unreachable";
    endpoints.insert("/v1/models".to_string(), models_ep);

    // Optional probes: skip entirely when the host is unreachable (black-holed)
    // so we do not pay a full client timeout four more times; otherwise run them
    // concurrently so worst-case latency stays ~one timeout.
    let (engine, health) = if primary_unreachable {
        for path in ["/version", "/get_model_info", "/health", "/metrics"] {
            endpoints.insert(
                path.to_string(),
                EndpointInfo {
                    status: "unreachable".into(),
                    http_status: None,
                },
            );
        }
        (
            EngineInfo {
                kind: "unknown".into(),
                version: None,
            },
            HealthInfo {
                status: "unreachable".into(),
                latency_ms: None,
            },
        )
    } else {
        let (version, model_info, health, metrics) = tokio::join!(
            fetch(client, "/version"),
            fetch(client, "/get_model_info"),
            health_probe(client),
            fetch_metrics(client),
        );
        endpoints.insert("/version".to_string(), version.0);
        endpoints.insert("/get_model_info".to_string(), model_info.0);
        endpoints.insert("/health".to_string(), health.1);
        endpoints.insert("/metrics".to_string(), metrics);
        let classified = classify_engine(version.1.as_deref(), model_info.1.as_deref());
        (
            EngineInfo {
                kind: engine_kind(models_ok, &classified.kind),
                version: classified.version,
            },
            health.0,
        )
    };

    let ok = probe_ok(&auth.status, models.len());

    let mut errors = Vec::new();
    for (path, ep) in &endpoints {
        let meaningful = path == "/v1/models"
            || matches!(ep.status.as_str(), "denied" | "unreachable" | "error");
        if ep.status != "ok" && meaningful {
            let detail = ep
                .http_status
                .map(|c| c.to_string())
                .unwrap_or_else(|| "unreachable".to_string());
            errors.push(ProbeError {
                probe: path.clone(),
                message: detail,
            });
        }
    }

    ProbeReport {
        probe_version: 1,
        ok,
        target,
        url,
        auth,
        engine,
        models,
        latency_ms,
        health,
        endpoints,
        errors,
    }
}

async fn fetch(client: &Client, path: &str) -> (EndpointInfo, Option<String>) {
    match client.get(path).await {
        Ok(b) => (
            EndpointInfo {
                status: "ok".into(),
                http_status: Some(200),
            },
            Some(b),
        ),
        Err(Error::Http { status, .. }) => (
            EndpointInfo {
                status: endpoint_status(status).into(),
                http_status: Some(status),
            },
            None,
        ),
        Err(_) => (
            EndpointInfo {
                status: "unreachable".into(),
                http_status: None,
            },
            None,
        ),
    }
}

async fn health_probe(client: &Client) -> (HealthInfo, EndpointInfo) {
    let h0 = Instant::now();
    match client.get("/health").await {
        Ok(_) => (
            HealthInfo {
                status: "ok".into(),
                latency_ms: Some(h0.elapsed().as_millis() as u64),
            },
            EndpointInfo {
                status: "ok".into(),
                http_status: Some(200),
            },
        ),
        Err(Error::Http { status, .. }) => (
            HealthInfo {
                status: "missing".into(),
                latency_ms: None,
            },
            EndpointInfo {
                status: endpoint_status(status).into(),
                http_status: Some(status),
            },
        ),
        Err(_) => (
            HealthInfo {
                status: "unreachable".into(),
                latency_ms: None,
            },
            EndpointInfo {
                status: "unreachable".into(),
                http_status: None,
            },
        ),
    }
}

async fn fetch_metrics(client: &Client) -> EndpointInfo {
    match client.get("/metrics").await {
        Ok(_) => EndpointInfo {
            status: "ok".into(),
            http_status: Some(200),
        },
        Err(Error::Http {
            status: 401 | 403, ..
        }) => match client.get_unauthenticated("/metrics").await {
            Ok(_) => EndpointInfo {
                status: "ok".into(),
                http_status: Some(200),
            },
            Err(Error::Http { status, .. }) => EndpointInfo {
                status: endpoint_status(status).into(),
                http_status: Some(status),
            },
            Err(_) => EndpointInfo {
                status: "unreachable".into(),
                http_status: None,
            },
        },
        Err(Error::Http { status, .. }) => EndpointInfo {
            status: endpoint_status(status).into(),
            http_status: Some(status),
        },
        Err(_) => EndpointInfo {
            status: "unreachable".into(),
            http_status: None,
        },
    }
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
        assert_eq!(auth_status(true, Some(200)), "ok");
        assert_eq!(auth_status(false, Some(200)), "not_required");
        assert_eq!(auth_status(true, Some(401)), "rejected");
        assert_eq!(auth_status(false, Some(401)), "required_missing");
        assert_eq!(auth_status(true, Some(403)), "rejected");
        assert_eq!(auth_status(false, Some(403)), "required_missing");
        assert_eq!(auth_status(true, Some(500)), "error");
        assert_eq!(auth_status(true, None), "unreachable");
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
        let vllm = classify_engine(Some(r#"{"version":"0.32"}"#), None);
        assert_eq!(vllm.kind, "vllm");
        assert_eq!(vllm.version.as_deref(), Some("0.32"));

        let sglang = classify_engine(None, Some(r#"{"model_path":"x"}"#));
        assert_eq!(sglang.kind, "sglang");

        assert_eq!(
            classify_engine(Some("nope"), Some("nope")).kind,
            "openai-compatible"
        );

        let both = classify_engine(Some(r#"{"version":"0.32"}"#), Some(r#"{"model_path":"x"}"#));
        assert_eq!(both.kind, "vllm");

        assert_eq!(classify_engine(None, None).kind, "openai-compatible");
    }

    #[test]
    fn engine_kind_unknown_only_when_incompatible() {
        assert_eq!(engine_kind(true, "openai-compatible"), "openai-compatible");
        assert_eq!(engine_kind(false, "openai-compatible"), "unknown");
        assert_eq!(engine_kind(false, "vllm"), "vllm");
        assert_eq!(engine_kind(true, "sglang"), "sglang");
    }

    #[test]
    fn probe_ok_rule() {
        assert!(probe_ok("ok", 1));
        assert!(probe_ok("not_required", 2));
        assert!(!probe_ok("rejected", 1));
        assert!(!probe_ok("ok", 0));
    }
}
