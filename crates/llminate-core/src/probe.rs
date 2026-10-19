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

/// Probe one endpoint and build its report. Never panics; optional probes are
/// best-effort and recorded in `endpoints` / `errors`.
pub async fn probe(client: &Client, target: Option<String>) -> ProbeReport {
    let has_key = client.has_api_key();
    let url = client.base_url().to_string();
    let mut endpoints = BTreeMap::new();
    let mut errors = Vec::new();

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
    endpoints.insert("/v1/models".to_string(), models_ep.clone());

    // engine (best-effort); mark unknown when the endpoint is not OpenAI-compatible
    let version_body = fetch(client, "/version", &mut endpoints).await;
    let model_info_body = fetch(client, "/get_model_info", &mut endpoints).await;
    let mut engine = classify_engine(version_body.as_deref(), model_info_body.as_deref());
    if models_ep.status != "ok" && engine.kind == "openai-compatible" {
        engine.kind = "unknown".to_string();
    }

    // health (best-effort, timed)
    let h0 = Instant::now();
    let (health, health_ep) = match client.get("/health").await {
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
    };
    endpoints.insert("/health".to_string(), health_ep);

    // metrics (best-effort, retry unauthenticated on 401/403)
    endpoints.insert("/metrics".to_string(), fetch_metrics(client).await);

    let ok = matches!(auth.status.as_str(), "ok" | "not_required") && !models.is_empty();

    for (path, ep) in &endpoints {
        if ep.status != "ok" {
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

async fn fetch(
    client: &Client,
    path: &str,
    endpoints: &mut BTreeMap<String, EndpointInfo>,
) -> Option<String> {
    match client.get(path).await {
        Ok(b) => {
            endpoints.insert(
                path.to_string(),
                EndpointInfo {
                    status: "ok".into(),
                    http_status: Some(200),
                },
            );
            Some(b)
        }
        Err(Error::Http { status, .. }) => {
            endpoints.insert(
                path.to_string(),
                EndpointInfo {
                    status: endpoint_status(status).into(),
                    http_status: Some(status),
                },
            );
            None
        }
        Err(_) => {
            endpoints.insert(
                path.to_string(),
                EndpointInfo {
                    status: "unreachable".into(),
                    http_status: None,
                },
            );
            None
        }
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
}
