//! Subcommand implementations.

pub mod probe;

use crate::output::{mask_key, pretty_json, Style};
use llminate_core::config::{self, Config};
use llminate_core::{sse, BenchRow, BenchStats, Client, Error, ResolvedTarget, Result};
use serde_json::{json, Value};
use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Semaphore;

// ---------- helpers ----------

fn need_key(t: &ResolvedTarget) -> Result<()> {
    t.require_api_key().map(|_| ())
}

async fn resolve_model(client: &Client, model: &Option<String>) -> Result<String> {
    if let Some(m) = model {
        if !m.is_empty() {
            return Ok(m.clone());
        }
    }
    let body = client.get("/v1/models").await?;
    let v: Value = serde_json::from_str(&body)?;
    v["data"][0]["id"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| Error::msg("aucun modèle dans /v1/models"))
}

fn read_text(arg: Option<String>) -> Result<String> {
    if let Some(s) = arg {
        if !s.is_empty() {
            return Ok(s);
        }
    }
    if !std::io::stdin().is_terminal() {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
        if !buf.trim().is_empty() {
            return Ok(buf);
        }
    }
    Err(Error::msg("texte vide"))
}

// ---------- models ----------

pub async fn models(t: &ResolvedTarget, s: &Style, ids: bool, raw: bool) -> Result<()> {
    need_key(t)?;
    let client = Client::new(t)?;
    let body = client.get("/v1/models").await?;
    if raw {
        println!("{body}");
        return Ok(());
    }
    let v: Value = serde_json::from_str(&body)?;
    let arr = v["data"].as_array();
    if ids {
        if let Some(arr) = arr {
            for m in arr {
                if let Some(id) = m["id"].as_str() {
                    println!("{id}");
                }
            }
        }
    } else if let Some(arr) = arr {
        for m in arr {
            let id = m["id"].as_str().unwrap_or("?");
            let owner = m["owned_by"].as_str().unwrap_or("?");
            println!("{}  ({})", s.bold(id), owner);
        }
    } else {
        println!("{}", pretty_json(&body));
    }
    Ok(())
}

// ---------- chat ----------

pub struct ChatSpec {
    pub model: Option<String>,
    pub system: Option<String>,
    pub prompt: String,
    pub max_tokens: u32,
    pub temperature: f64,
    pub stream: bool,
}

fn chat_body(spec: &ChatSpec, model: &str) -> Value {
    let mut messages = Vec::new();
    if let Some(sys) = &spec.system {
        messages.push(json!({"role": "system", "content": sys}));
    }
    messages.push(json!({"role": "user", "content": spec.prompt}));
    json!({
        "model": model,
        "messages": messages,
        "max_tokens": spec.max_tokens,
        "temperature": spec.temperature,
        "stream": spec.stream,
    })
}

pub async fn chat_raw(t: &ResolvedTarget, spec: &ChatSpec) -> Result<String> {
    let client = Client::new(t)?;
    let model = resolve_model(&client, &spec.model).await?;
    let body = chat_body(spec, &model);
    client.post_json("/v1/chat/completions", &body).await
}

pub async fn chat_text(t: &ResolvedTarget, spec: &ChatSpec) -> Result<String> {
    let client = Client::new(t)?;
    let model = resolve_model(&client, &spec.model).await?;
    let body = chat_body(spec, &model);
    if spec.stream {
        stream_chat(&client, &body).await
    } else {
        let raw = client.post_json("/v1/chat/completions", &body).await?;
        let v: Value = serde_json::from_str(&raw)?;
        Ok(sse::content_from_message(&v).unwrap_or_default())
    }
}

async fn stream_chat(client: &Client, body: &Value) -> Result<String> {
    use futures_util::StreamExt;
    let resp = client.post_stream("/v1/chat/completions", body).await?;
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut out = String::new();
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(pos) = buf.find('\n') {
            let line: String = buf.drain(..=pos).collect();
            if let Some(delta) = sse::content_from_line(line.trim_end_matches('\n')) {
                let _ = lock.write_all(delta.as_bytes());
                let _ = lock.flush();
                out.push_str(&delta);
            }
        }
    }
    let _ = lock.write_all(b"\n");
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
pub async fn chat(
    t: &ResolvedTarget,
    message: Option<String>,
    model: Option<String>,
    system: Option<String>,
    max_tokens: u32,
    temperature: f64,
    no_stream: bool,
    raw: bool,
) -> Result<()> {
    need_key(t)?;
    let prompt = read_text(message)?;
    let spec = ChatSpec {
        model,
        system,
        prompt,
        max_tokens,
        temperature,
        stream: !no_stream,
    };
    if raw {
        let body = chat_raw(t, &spec).await?;
        println!("{body}");
    } else if spec.stream {
        chat_text(t, &spec).await?;
    } else {
        println!("{}", chat_text(t, &spec).await?);
    }
    Ok(())
}

// ---------- complete ----------

pub async fn complete(
    t: &ResolvedTarget,
    prompt: Option<String>,
    model: Option<String>,
    max_tokens: u32,
    temperature: f64,
    raw: bool,
) -> Result<()> {
    need_key(t)?;
    let prompt = read_text(prompt)?;
    let client = Client::new(t)?;
    let model = resolve_model(&client, &model).await?;
    let body = json!({
        "model": model,
        "prompt": prompt,
        "max_tokens": max_tokens,
        "temperature": temperature,
    });
    let out = client.post_json("/v1/completions", &body).await?;
    if raw {
        println!("{out}");
        return Ok(());
    }
    let v: Value = serde_json::from_str(&out)?;
    println!("{}", v["choices"][0]["text"].as_str().unwrap_or(""));
    Ok(())
}

// ---------- embed ----------

pub async fn embed(
    t: &ResolvedTarget,
    text: Option<String>,
    model: Option<String>,
    raw: bool,
) -> Result<()> {
    need_key(t)?;
    let text = read_text(text)?;
    let client = Client::new(t)?;
    let model = resolve_model(&client, &model).await?;
    let body = json!({"model": model, "input": text});
    let out = client.post_json("/v1/embeddings", &body).await?;
    if raw {
        println!("{out}");
        return Ok(());
    }
    let v: Value = serde_json::from_str(&out)?;
    let e = &v["data"][0]["embedding"];
    let dim = e.as_array().map(|a| a.len()).unwrap_or(0);
    let preview: Vec<String> = e
        .as_array()
        .map(|a| a.iter().take(4).map(|x| x.to_string()).collect())
        .unwrap_or_default();
    println!("dim={dim} preview=[{}]", preview.join(", "));
    Ok(())
}

// ---------- tokenize ----------

pub async fn tokenize(
    t: &ResolvedTarget,
    text: Option<String>,
    model: Option<String>,
    raw: bool,
) -> Result<()> {
    need_key(t)?;
    let text = read_text(text)?;
    let client = Client::new(t)?;
    let model = resolve_model(&client, &model).await?;
    let body = json!({"model": model, "prompt": text});
    let out = client.post_json("/tokenize", &body).await?;
    if raw {
        println!("{out}");
        return Ok(());
    }
    let v: Value = serde_json::from_str(&out)?;
    let count = v["count"].as_u64().unwrap_or(0);
    let mml = v["max_model_len"]
        .as_u64()
        .map(|n| n.to_string())
        .unwrap_or_else(|| "?".to_string());
    println!("tokens={count} max_model_len={mml}");
    Ok(())
}

// ---------- health ----------

pub async fn health(t: &ResolvedTarget, raw: bool, time: bool) -> Result<()> {
    need_key(t)?;
    let client = Client::new(t)?;
    let start = Instant::now();
    let body = client.get("/health").await?;
    let ms = start.elapsed().as_millis();
    if raw {
        println!("{body}");
        return Ok(());
    }
    println!("health: {} ({}ms via {})", body.trim(), ms, t.url);
    if time {
        let s2 = Instant::now();
        let _ = client.get("/health").await?;
        println!("total={}ms", s2.elapsed().as_millis());
    }
    Ok(())
}

// ---------- info ----------

pub async fn info(t: &ResolvedTarget, _s: &Style, raw: bool) -> Result<()> {
    need_key(t)?;
    let client = Client::new(t)?;
    let models = client.get_opt("/v1/models").await;
    let version = client.get_opt("/version").await;
    let model_info = client.get_opt("/get_model_info").await;
    if raw {
        println!(
            "== /v1/models\n{}",
            models.unwrap_or_else(|| "MISSING".into())
        );
        println!(
            "== /version\n{}",
            version.unwrap_or_else(|| "MISSING".into())
        );
        println!(
            "== /get_model_info\n{}",
            model_info.unwrap_or_else(|| "MISSING".into())
        );
        return Ok(());
    }
    println!("base: {}", t.url);
    match &models {
        Some(m) => {
            let v: Value = serde_json::from_str(m)?;
            let ids: Vec<String> = v["data"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x["id"].as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            println!("models: {}", ids.join(", "));
        }
        None => println!("models: missing"),
    }
    if let Some(v) = &version {
        let parsed: Value = serde_json::from_str(v)?;
        println!(
            "engine: vllm ({})",
            parsed["version"].as_str().unwrap_or("?")
        );
    } else if let Some(mi) = &model_info {
        let parsed: Value = serde_json::from_str(mi)?;
        println!(
            "engine: sglang ({})",
            parsed["model_path"].as_str().unwrap_or("?")
        );
    } else {
        println!("engine: openai-compatible (indéterminé)");
    }
    Ok(())
}

// ---------- metrics ----------

fn metric_value(body: &str, prefix: &str) -> Option<f64> {
    body.lines()
        .find(|l| l.starts_with(prefix))
        .and_then(|l| l.rsplit(' ').next())
        .and_then(|n| n.trim().parse::<f64>().ok())
}

pub async fn metrics(
    t: &ResolvedTarget,
    _s: &Style,
    raw: bool,
    filter: Option<String>,
) -> Result<()> {
    need_key(t)?;
    let client = Client::new(t)?;
    let mut body = match client.get("/metrics").await {
        Ok(b) => b,
        Err(_) => client.get_unauthenticated("/metrics").await?,
    };
    if let Some(f) = &filter {
        body = body
            .lines()
            .filter(|l| l.contains(f.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
    }
    if raw {
        println!("{body}");
        return Ok(());
    }
    let nv = body.lines().filter(|l| l.starts_with("vllm:")).count();
    let ns = body.lines().filter(|l| l.starts_with("sglang:")).count();
    let engine = if nv > ns {
        "vllm"
    } else if ns > nv {
        "sglang"
    } else {
        "unknown"
    };
    println!("engine: {engine} (base {})", t.url);
    if let Some(run) = metric_value(&body, "vllm:num_requests_running{")
        .or_else(|| metric_value(&body, "sglang:num_running_reqs{"))
    {
        println!("running={}", run as u64);
    }
    if let Some(wait) = metric_value(&body, "vllm:num_requests_waiting{")
        .or_else(|| metric_value(&body, "sglang:num_queue_reqs{"))
    {
        println!("waiting={}", wait as u64);
    }
    if let Some(kv) = metric_value(&body, "vllm:kv_cache_usage_perc{") {
        println!("kv_cache={:.0}%", kv * 100.0);
    } else if let Some(tu) = metric_value(&body, "sglang:token_usage{") {
        println!("token_usage={:.0}%", tu * 100.0);
    }
    if let Some(pt) = metric_value(&body, "vllm:prompt_tokens_total{")
        .or_else(|| metric_value(&body, "sglang:prompt_tokens_total{"))
    {
        println!("prompt_tokens={:.0}", pt);
    }
    if let Some(gt) = metric_value(&body, "vllm:generation_tokens_total{")
        .or_else(|| metric_value(&body, "sglang:generation_tokens_total{"))
    {
        println!("generation_tokens={:.0}", gt);
    }
    if let Some(hr) = metric_value(&body, "sglang:cache_hit_rate{") {
        println!("cache_hit_rate={:.1}%", hr * 100.0);
    }
    Ok(())
}

// ---------- smoke ----------

pub async fn smoke(t: &ResolvedTarget, prompt: String, model: Option<String>) -> Result<()> {
    need_key(t)?;
    let client = Client::new(t)?;
    client
        .get("/health")
        .await
        .map_err(|e| Error::msg(format!("SMOKE KO: /health injoignable ({e})")))?;
    let model = resolve_model(&client, &model).await?;
    let start = Instant::now();
    let spec = ChatSpec {
        model: Some(model.clone()),
        system: None,
        prompt,
        max_tokens: 5,
        temperature: 0.7,
        stream: false,
    };
    let raw = chat_raw(t, &spec)
        .await
        .map_err(|e| Error::msg(format!("SMOKE KO: chat échoué ({e})")))?;
    let ms = start.elapsed().as_millis();
    let v: Value = serde_json::from_str(&raw)?;
    let text = sse::content_from_message(&v).unwrap_or_default();
    if text.is_empty() {
        return Err(Error::msg("SMOKE KO: réponse vide"));
    }
    let preview: String = text.chars().take(80).collect();
    println!("SMOKE OK model={model} latency_ms={ms} reply={preview}");
    Ok(())
}

// ---------- bench ----------

pub async fn bench(
    t: &ResolvedTarget,
    n: u32,
    concurrency: u32,
    prompt: String,
    max_tokens: u32,
    model: Option<String>,
) -> Result<()> {
    need_key(t)?;
    if n == 0 {
        return Err(Error::Config("--n doit être un entier >= 1".into()));
    }
    let client = Client::new(t)?;
    let model = resolve_model(&client, &model).await?;
    let conc = concurrency.max(1);
    let sem = Arc::new(Semaphore::new(conc as usize));
    let start = Instant::now();
    let mut handles = Vec::new();
    for _ in 0..n {
        let permit = sem.clone().acquire_owned().await.map_err(Error::msg)?;
        let client = client.clone();
        let model = model.clone();
        let prompt = prompt.clone();
        handles.push(tokio::spawn(async move {
            let _permit = permit;
            let body = json!({
                "model": model,
                "messages": [{"role": "user", "content": prompt}],
                "max_tokens": max_tokens,
                "temperature": 0.7,
                "stream": false,
            });
            let t0 = Instant::now();
            let resp = client.post_json("/v1/chat/completions", &body).await;
            let latency_ms = t0.elapsed().as_millis() as u64;
            match resp {
                Ok(b) => {
                    let tok = serde_json::from_str::<Value>(&b)
                        .ok()
                        .and_then(|v| v["usage"]["completion_tokens"].as_u64())
                        .unwrap_or(0);
                    Ok(BenchRow {
                        latency_ms,
                        completion_tokens: tok,
                    })
                }
                Err(e) => Err(e.to_string()),
            }
        }));
    }
    let mut rows = Vec::new();
    let mut failures = 0usize;
    for h in handles {
        match h.await {
            Ok(Ok(row)) => rows.push(row),
            _ => failures += 1,
        }
    }
    let wall = start.elapsed().as_secs_f64();
    if rows.is_empty() {
        return Err(Error::msg(format!("bench: les {n} requêtes ont échoué")));
    }
    let stats = BenchStats::new(rows);
    println!(
        "n={} concurrency={conc} model={model} failed={failures}",
        stats.n()
    );
    println!(
        "latency_ms: p50={} p95={} min={} max={}",
        stats.p50(),
        stats.p95(),
        stats.min(),
        stats.max()
    );
    println!(
        "completion_tokens={} tok/s={:.1} req/s={:.2}",
        stats.completion_tokens(),
        stats.tokens_per_second(wall),
        stats.requests_per_second(wall)
    );
    Ok(())
}

// ---------- targets / config ----------

pub fn targets(s: &Style) -> Result<()> {
    let cfg = config::load()?;
    let path = config::config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(introuvable)".to_string());
    if cfg.targets.is_empty() {
        println!("(aucune cible configurée)");
        println!("config: {path}");
        return Ok(());
    }
    let def = cfg.default_target.as_deref();
    for (name, tgt) in &cfg.targets {
        let marker = if Some(name.as_str()) == def { "*" } else { " " };
        let key = format!("key:{}", config::key_source_label(tgt));
        println!("{marker} {}  {}  {}", s.bold(name), tgt.url, key);
    }
    println!("config: {path}");
    Ok(())
}

pub fn use_target(name: &str) -> Result<()> {
    let cfg = config::load()?;
    if !cfg.targets.contains_key(name) {
        return Err(Error::Config(format!(
            "cible inconnue '{name}' (connues: {})",
            cfg.targets.keys().cloned().collect::<Vec<_>>().join(", ")
        )));
    }
    let path = config::config_path()
        .ok_or_else(|| Error::Config("chemin de config introuvable (HOME ?)".into()))?;
    config::set_default_target(&path, name)?;
    println!("default_target = {name} ({})", path.display());
    Ok(())
}

pub fn config(t: &ResolvedTarget, _cfg: &Config, _s: &Style) -> Result<()> {
    println!("target={}", t.display_name());
    println!("url={}", t.url);
    println!(
        "model={}",
        t.model
            .clone()
            .unwrap_or_else(|| "(auto: premier id de /v1/models)".to_string())
    );
    println!("timeout={}", t.timeout);
    let key = t
        .api_key
        .as_deref()
        .map(mask_key)
        .unwrap_or_else(|| "(absente)".to_string());
    println!("api_key={key}");
    if let Some(p) = config::config_path() {
        println!("config={}", p.display());
    }
    Ok(())
}

pub fn completions(shell: clap_complete::Shell) {
    use clap::CommandFactory;
    let mut cmd = crate::cli::Cli::command();
    clap_complete::generate(shell, &mut cmd, "llminate", &mut std::io::stdout());
}
