//! End-to-end tests: a local OpenAI-compatible stub server drives the CLI binary.

use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::process::Command;
use std::sync::OnceLock;

const METRICS: &str = "\
# HELP vllm:num_requests_running Number of requests in model execution batches.\n\
# TYPE vllm:num_requests_running gauge\n\
vllm:num_requests_running{model_name=\"test-model-A\"} 3.0\n\
# TYPE vllm:num_requests_waiting gauge\n\
vllm:num_requests_waiting{model_name=\"test-model-A\"} 1.0\n\
# TYPE vllm:kv_cache_usage_perc gauge\n\
vllm:kv_cache_usage_perc{model_name=\"test-model-A\"} 0.42\n\
# TYPE vllm:prompt_tokens_total counter\n\
vllm:prompt_tokens_total{model_name=\"test-model-A\"} 1234.0\n\
# TYPE vllm:generation_tokens_total counter\n\
vllm:generation_tokens_total{model_name=\"test-model-A\"} 5678.0\n\
# TYPE sglang:token_usage gauge\n\
sglang:token_usage{model_name=\"test-model-A\"} 0.28\n";

fn start(app: Router) -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind");
            let port = listener.local_addr().expect("addr").port();
            tx.send(port).expect("send port");
            axum::serve(listener, app).await.expect("serve");
        });
    });
    format!("http://127.0.0.1:{}", rx.recv().expect("recv port"))
}

fn base() -> &'static str {
    static BASE: OnceLock<String> = OnceLock::new();
    BASE.get_or_init(|| start(app()))
}

async fn models() -> Json<Value> {
    Json(json!({"object": "list", "data": [
        {"id": "test-model-A", "object": "model", "owned_by": "test"},
        {"id": "test-model-B", "object": "model", "owned_by": "test"}
    ]}))
}

async fn health() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

async fn version() -> Json<Value> {
    Json(json!({"version": "0.0-test"}))
}

async fn model_info() -> Json<Value> {
    Json(json!({"model_path": "test-model-A", "is_generation": true}))
}

async fn metrics() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/plain")], METRICS)
}

async fn chat(Json(body): Json<Value>) -> impl IntoResponse {
    if body["model"] == "reason-model" {
        return Json(json!({
            "choices": [{"message": {"role": "assistant", "content": null, "reasoning": "think-think"}, "finish_reason": "stop"}],
            "usage": {"completion_tokens": 5}
        }))
        .into_response();
    }
    if body["stream"] == true {
        let sse = "data: {\"choices\": [{\"delta\": {\"content\": \"Hello\"}}]}\n\ndata\n";
        return ([(header::CONTENT_TYPE, "text/event-stream")], sse).into_response();
    }
    Json(json!({"choices": [{"message": {"content": "Hello-test"}}], "usage": {"completion_tokens": 2}})).into_response()
}

async fn complete() -> Json<Value> {
    Json(json!({"choices": [{"text": "Complete-test"}], "usage": {"completion_tokens": 2}}))
}

async fn embed() -> Json<Value> {
    Json(json!({"data": [{"embedding": [0.1, 0.2, 0.3, 0.4]}]}))
}

async fn tokenize(Json(b): Json<Value>) -> Json<Value> {
    let n = b["prompt"].as_str().unwrap_or("").len() / 4 + 1;
    Json(json!({"tokens": [0], "count": n, "max_model_len": 131072}))
}

fn app() -> Router {
    Router::new()
        .route("/v1/models", get(models))
        .route("/health", get(health))
        .route("/version", get(version))
        .route("/get_model_info", get(model_info))
        .route("/metrics", get(metrics))
        .route("/v1/chat/completions", post(chat))
        .route("/v1/completions", post(complete))
        .route("/v1/embeddings", post(embed))
        .route("/tokenize", post(tokenize))
}

fn cfg_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("llminate-it-{}", std::process::id()))
}

async fn not_compatible_models() -> axum::response::Response {
    (axum::http::StatusCode::NOT_FOUND, "nope").into_response()
}

fn not_compatible_app() -> Router {
    // Answers, but exposes no OpenAI-compatible /v1/models and no engine marker
    // (/version, /get_model_info), so the probe must classify it as unknown.
    Router::new()
        .route("/v1/models", get(not_compatible_models))
        .route("/health", get(health))
        .route("/metrics", get(metrics))
}

fn base_not_compatible() -> &'static str {
    static BASE: OnceLock<String> = OnceLock::new();
    BASE.get_or_init(|| start(not_compatible_app()))
}

async fn metrics_flip(headers: axum::http::HeaderMap) -> axum::response::Response {
    if headers.contains_key(axum::http::header::AUTHORIZATION) {
        (axum::http::StatusCode::UNAUTHORIZED, "no").into_response()
    } else {
        ([(axum::http::header::CONTENT_TYPE, "text/plain")], METRICS).into_response()
    }
}

fn dead_url() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    format!("http://127.0.0.1:{port}")
}

fn run(args: &[&str], with_key: bool) -> std::process::Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_llminate"));
    c.env("LLMINATE_CONFIG", cfg_path())
        .env("LLMINATE_BASE_URL", base());
    if with_key {
        c.env("LLMINATE_API_KEY", "sk-test");
    } else {
        c.env_remove("LLMINATE_API_KEY");
    }
    c.args(args).output().unwrap()
}

fn run_key(args: &[&str], key: &str) -> std::process::Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_llminate"));
    c.env("LLMINATE_CONFIG", cfg_path())
        .env("LLMINATE_BASE_URL", base())
        .env("LLMINATE_API_KEY", key)
        .args(args)
        .output()
        .unwrap()
}

async fn auth_models(headers: axum::http::HeaderMap) -> axum::response::Response {
    let ok = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        == Some("Bearer sk-test");
    if ok {
        Json(json!({"object": "list", "data": [
            {"id": "auth-model", "object": "model", "owned_by": "vllm"}]}))
        .into_response()
    } else {
        (axum::http::StatusCode::UNAUTHORIZED, "unauthorized").into_response()
    }
}

fn auth_app() -> Router {
    Router::new()
        .route("/v1/models", get(auth_models))
        .route("/health", get(health))
        .route("/version", get(version))
        .route("/get_model_info", get(model_info))
        .route("/metrics", get(metrics_flip))
}

fn base_auth() -> &'static str {
    static BASE: OnceLock<String> = OnceLock::new();
    BASE.get_or_init(|| start(auth_app()))
}

fn run_at(args: &[&str], base: &str, key: Option<&str>) -> std::process::Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_llminate"));
    c.env("LLMINATE_CONFIG", cfg_path())
        .env("LLMINATE_BASE_URL", base);
    match key {
        Some(k) => {
            c.env("LLMINATE_API_KEY", k);
        }
        None => {
            c.env_remove("LLMINATE_API_KEY");
        }
    }
    c.args(args).output().unwrap()
}

fn out(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn help_needs_no_key() {
    let o = run(&["--help"], false);
    assert!(o.status.success());
    assert!(out(&o).contains("Usage"), "{}", out(&o));
}

#[test]
fn missing_key_exit_2() {
    let o = run(&["models"], false);
    assert_eq!(o.status.code(), Some(2));
    assert!(err(&o).contains("API"), "{}", err(&o));
}

#[test]
fn models_lists_ids() {
    let o = run(&["models"], true);
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("test-model-A"));
}

#[test]
fn models_ids_flag() {
    let o = run(&["models", "--ids"], true);
    let body = out(&o);
    let lines: Vec<&str> = body.trim().lines().collect();
    assert_eq!(lines, vec!["test-model-A", "test-model-B"]);
}

#[test]
fn config_masks_key() {
    let o = run_key(&["config"], "sk-abcdefghij123456");
    let s = out(&o);
    assert!(!s.contains("sk-abcdefghij123456"), "{s}");
    assert!(s.contains("3456"), "{s}");
    assert!(s.contains("127.0.0.1"), "{s}");
}

#[test]
fn chat_returns_text() {
    let o = run(&["chat", "bonjour", "--no-stream"], true);
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("Hello-test"));
}

#[test]
fn chat_stream_decodes_sse() {
    let o = run(&["chat", "bonjour"], true);
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(out(&o).replace('\n', "").trim(), "Hello");
}

#[test]
fn chat_stdin_pipe() {
    use std::io::Write as _;
    let mut c = Command::new(env!("CARGO_BIN_EXE_llminate"));
    c.env("LLMINATE_CONFIG", cfg_path())
        .env("LLMINATE_BASE_URL", base())
        .env("LLMINATE_API_KEY", "sk-test")
        .args(["chat", "--no-stream"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped());
    let mut child = c.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"question\n")
        .unwrap();
    let o = child.wait_with_output().unwrap();
    assert!(out(&o).contains("Hello-test"));
}

#[test]
fn complete_returns_text() {
    let o = run(&["complete", "il etait"], true);
    assert!(out(&o).contains("Complete-test"));
}

#[test]
fn health_ok() {
    let o = run(&["health"], true);
    assert!(out(&o).to_lowercase().contains("ok"));
}

#[test]
fn info_shows_version() {
    let o = run(&["info"], true);
    assert!(out(&o).contains("0.0-test"));
}

#[test]
fn embed_dim() {
    let o = run(&["embed", "bonjour"], true);
    assert!(out(&o).contains("dim=4"));
}

#[test]
fn tokenize_count() {
    let o = run(&["tokenize", "bonjour monde"], true);
    assert!(out(&o).contains("tokens="));
}

#[test]
fn metrics_summary() {
    let o = run(&["metrics"], true);
    let s = out(&o);
    for k in [
        "running=3",
        "waiting=1",
        "kv_cache=42%",
        "prompt_tokens=1234",
        "generation_tokens=5678",
    ] {
        assert!(s.contains(k), "missing {k} in:\n{s}");
    }
}

#[test]
fn metrics_filter() {
    let o = run(&["metrics", "--filter", "kv_cache"], true);
    let s = out(&o);
    assert!(s.contains("kv_cache"));
    assert!(!s.contains("num_requests_running"));
}

#[test]
fn smoke_ok() {
    let o = run(&["smoke"], true);
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("SMOKE OK"));
}

#[test]
fn bench_small() {
    let o = run(&["bench", "--n", "3", "--max-tokens", "2"], true);
    let s = out(&o);
    assert!(s.contains("n=3"), "{s}");
    assert!(s.contains("p50="), "{s}");
}

#[test]
fn bench_bad_n() {
    let o = run(&["bench", "--n", "0"], true);
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn reasoning_fallback() {
    let o = run(
        &["chat", "dis OK", "--no-stream", "-m", "reason-model"],
        true,
    );
    assert!(out(&o).contains("think-think"));
}

#[test]
fn targets_list_names() {
    let dir = std::env::temp_dir().join(format!("llminate-cfg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let body = "default_target = \"spark\"\n[targets.spark]\nurl = \"http://1.1.1.1:1\"\n[targets.lab]\nurl = \"http://2.2.2.2:2\"\n";
    std::fs::write(&path, body).unwrap();
    let mut c = Command::new(env!("CARGO_BIN_EXE_llminate"));
    c.env("LLMINATE_CONFIG", &path);
    let o = c.arg("targets").output().unwrap();
    let s = out(&o);
    assert!(s.contains("spark"), "{s}");
    assert!(s.contains("lab"), "{s}");
    assert!(s.contains('*'), "{s}");
}

#[test]
fn use_sets_default_target() {
    let dir = std::env::temp_dir().join(format!("llminate-use-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let body =
        "[targets.spark]\nurl = \"http://1.1.1.1:1\"\n[targets.lab]\nurl = \"http://2.2.2.2:2\"\n";
    std::fs::write(&path, body).unwrap();
    let mut c = Command::new(env!("CARGO_BIN_EXE_llminate"));
    c.env("LLMINATE_CONFIG", &path).args(["use", "lab"]);
    let o = c.output().unwrap();
    assert!(o.status.success(), "{}", err(&o));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("default_target = \"lab\""), "{text}");
}

#[test]
fn probe_json_contract() {
    let o = run_at(&["probe", "--json"], base(), Some("sk-test"));
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let v: Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(v["probe_version"], 1);
    assert_eq!(v["ok"], true);
    assert_eq!(v["auth"]["status"], "ok");
    assert_eq!(v["engine"]["kind"], "vllm");
    assert_eq!(v["engine"]["version"], "0.0-test");
    assert_eq!(v["models"][0]["id"], "test-model-A");
    assert_eq!(v["endpoints"]["/v1/models"]["status"], "ok");
    assert_eq!(v["endpoints"]["/metrics"]["status"], "ok");
}

#[test]
fn probe_auth_rejected() {
    let o = run_at(&["probe", "--json"], base_auth(), Some("sk-wrong"));
    assert_eq!(o.status.code(), Some(1), "{}", err(&o));
    let v: Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(v["auth"]["status"], "rejected");
    assert_eq!(v["ok"], false);
}

#[test]
fn probe_auth_required_missing() {
    let o = run_at(&["probe", "--json"], base_auth(), None);
    assert_eq!(o.status.code(), Some(1), "{}", err(&o));
    let v: Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(v["auth"]["status"], "required_missing");
}

#[test]
fn probe_human_ok() {
    let o = run_at(&["probe"], base(), Some("sk-test"));
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(out(&o).contains("=> OK"));
}

#[test]
fn probe_save_as_writes_target() {
    let dir = std::env::temp_dir().join(format!("llminate-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let mut c = Command::new(env!("CARGO_BIN_EXE_llminate"));
    c.env("LLMINATE_CONFIG", &path)
        .env("LLMINATE_BASE_URL", base())
        .env("LLMINATE_API_KEY", "sk-test")
        .args(["probe", "--save-as", "lab"]);
    let o = c.output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("[targets.lab]"), "{text}");
    assert!(text.contains(base()), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn probe_auth_ok_sends_bearer() {
    let o = run_at(&["probe", "--json"], base_auth(), Some("sk-test"));
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let v: Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(v["auth"]["status"], "ok");
    assert_eq!(v["models"][0]["id"], "auth-model");
    // /v1/models 200 with the key proves the bearer was transmitted;
    // /metrics ok proves the 401 -> unauthenticated retry worked.
    assert_eq!(v["endpoints"]["/metrics"]["status"], "ok");
}

#[test]
fn probe_not_openai_compatible() {
    let o = run_at(&["probe", "--json"], base_not_compatible(), Some("sk-test"));
    assert_eq!(o.status.code(), Some(1), "{}", err(&o));
    let v: Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(v["endpoints"]["/v1/models"]["status"], "missing");
    assert_eq!(v["engine"]["kind"], "unknown");
    assert_eq!(v["ok"], false);
}

#[test]
fn probe_unreachable_endpoint() {
    let url = dead_url();
    let o = run_at(&["probe", "--json"], &url, Some("sk-test"));
    assert_eq!(o.status.code(), Some(1), "{}", err(&o));
    let v: Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(v["auth"]["status"], "unreachable");
    assert_eq!(v["engine"]["kind"], "unknown");
}

#[test]
fn probe_save_as_failure_writes_nothing() {
    let dir = std::env::temp_dir().join(format!("llminate-probenowrite-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let mut c = Command::new(env!("CARGO_BIN_EXE_llminate"));
    c.env("LLMINATE_CONFIG", &path)
        .env("LLMINATE_BASE_URL", base_not_compatible())
        .env("LLMINATE_API_KEY", "sk-test")
        .args(["probe", "--save-as", "lab"]);
    let o = c.output().unwrap();
    assert_eq!(o.status.code(), Some(1), "{}", err(&o));
    assert!(
        !path.exists(),
        "config must not be written on a failed probe"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
