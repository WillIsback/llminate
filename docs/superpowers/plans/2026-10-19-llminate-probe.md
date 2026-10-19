# llminate probe Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `llminate probe` — inspect one OpenAI-compatible endpoint and emit a stable JSON report (plus an interactive wizard), for endpoint onboarding in third-party backends.

**Architecture:** Probe logic lives in `llminate-core` (`probe.rs`) so Rust apps can embed it; the CLI crate adds rendering (JSON / progressive / `dialoguer` wizard). Classification helpers are pure functions, unit-tested; the end-to-end contract is tested against the existing axum stub.

**Tech Stack:** Rust 2021, tokio, reqwest (rustls), serde/serde_json, clap, dialoguer (new), axum (dev).

**Spec:** `docs/specs/probe.md`.

---

## File structure

- `crates/llminate-core/src/probe.rs` — NEW. `ProbeReport` (+ sub-structs), pure classifiers, `probe()`.
- `crates/llminate-core/src/lib.rs` — MODIFY. `pub mod probe;` + re-export.
- `crates/llminate-core/src/client.rs` — MODIFY. add `Client::has_api_key()`.
- `crates/llminate/Cargo.toml` — MODIFY. add `dialoguer`.
- `Cargo.toml` (workspace) — MODIFY. add `dialoguer` to `[workspace.dependencies]`, bump version 0.3.0.
- `crates/llminate/src/cli.rs` — MODIFY. add `Probe` subcommand.
- `crates/llminate/src/commands.rs` — MODIFY. `pub mod probe;` + dispatch call.
- `crates/llminate/src/commands/probe.rs` — NEW. rendering + wizard.
- `crates/llminate/tests/integration.rs` — MODIFY. probe tests + auth stub.
- `README.md`, `docs/design.md` — MODIFY. document `probe`.

---

## Task 1: core types + pure classifiers

**Files:**
- Create: `crates/llminate-core/src/probe.rs`
- Modify: `crates/llminate-core/src/lib.rs`

- [ ] **Step 1: Create `probe.rs` with types and stubs + unit tests**

```rust
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
```

- [ ] **Step 2: Register the module in `crates/llminate-core/src/lib.rs`**

```rust
pub mod probe;
```
Add next to the other `pub mod` lines, and add to the re-exports:
```rust
pub use probe::ProbeReport;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p llminate-core probe`
Expected: 4 tests pass.

- [ ] **Step 4: Clippy**

Run: `cargo clippy -p llminate-core -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/llminate-core/src/probe.rs crates/llminate-core/src/lib.rs
git commit -m "feat(core): probe report types and classifiers"
```

## Task 2: `Client::has_api_key` + `probe()` orchestrator

**Files:**
- Modify: `crates/llminate-core/src/client.rs`
- Modify: `crates/llminate-core/src/probe.rs`

- [ ] **Step 1: Add `has_api_key` to `Client` (`client.rs`)**

Add inside `impl Client`, next to `base_url`:

```rust
    /// Whether a non-empty API key is configured.
    pub fn has_api_key(&self) -> bool {
        self.api_key.as_deref().map(|k| !k.is_empty()).unwrap_or(false)
    }
```

- [ ] **Step 2: Add imports + `probe()` to `probe.rs`**

At the top of `probe.rs`, extend the imports:

```rust
use crate::client::Client;
use crate::error::Error;
use std::time::Instant;
```

Append the orchestrator (after the classifiers, before `#[cfg(test)]`):

```rust
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
            AuthInfo { status: auth_status(has_key, true, Some(200)).into(), http_status: Some(200) },
            parse_models(body),
            EndpointInfo { status: "ok".into(), http_status: Some(200) },
        ),
        Err(Error::Http { status, .. }) => (
            AuthInfo { status: auth_status(has_key, false, Some(*status)).into(), http_status: Some(*status) },
            Vec::new(),
            EndpointInfo { status: endpoint_status(*status).into(), http_status: Some(*status) },
        ),
        Err(_) => (
            AuthInfo { status: auth_status(has_key, false, None).into(), http_status: None },
            Vec::new(),
            EndpointInfo { status: "unreachable".into(), http_status: None },
        ),
    };
    endpoints.insert("/v1/models".to_string(), models_ep.clone());
    if models_ep.status != "ok" {
        let detail = models_ep
            .http_status
            .map(|c| c.to_string())
            .unwrap_or_else(|| "unreachable".to_string());
        errors.push(ProbeError { probe: "/v1/models".to_string(), message: detail });
    }

    // engine (best-effort)
    let version_body = fetch(client, "/version", &mut endpoints).await;
    let model_info_body = fetch(client, "/get_model_info", &mut endpoints).await;
    let engine = classify_engine(version_body.as_deref(), model_info_body.as_deref());

    // health (best-effort, timed)
    let h0 = Instant::now();
    let (health, health_ep) = match client.get("/health").await {
        Ok(_) => (
            HealthInfo { status: "ok".into(), latency_ms: Some(h0.elapsed().as_millis() as u64) },
            EndpointInfo { status: "ok".into(), http_status: Some(200) },
        ),
        Err(Error::Http { status, .. }) => (
            HealthInfo { status: "missing".into(), latency_ms: None },
            EndpointInfo { status: endpoint_status(status).into(), http_status: Some(status) },
        ),
        Err(_) => (
            HealthInfo { status: "unreachable".into(), latency_ms: None },
            EndpointInfo { status: "unreachable".into(), http_status: None },
        ),
    };
    endpoints.insert("/health".to_string(), health_ep);

    // metrics (best-effort, retry unauthenticated on 401/403)
    endpoints.insert("/metrics".to_string(), fetch_metrics(client).await);

    let ok = matches!(auth.status.as_str(), "ok" | "not_required") && !models.is_empty();
    for (path, ep) in &endpoints {
        if ep.status != "ok" && path != "/v1/models" {
            let detail = ep.http_status.map(|c| c.to_string()).unwrap_or_else(|| "unreachable".to_string());
            errors.push(ProbeError { probe: path.clone(), message: detail });
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

async fn fetch(client: &Client, path: &str, endpoints: &mut BTreeMap<String, EndpointInfo>) -> Option<String> {
    match client.get(path).await {
        Ok(b) => {
            endpoints.insert(path.to_string(), EndpointInfo { status: "ok".into(), http_status: Some(200) });
            Some(b)
        }
        Err(Error::Http { status, .. }) => {
            endpoints.insert(path.to_string(), EndpointInfo { status: endpoint_status(status).into(), http_status: Some(status) });
            None
        }
        Err(_) => {
            endpoints.insert(path.to_string(), EndpointInfo { status: "unreachable".into(), http_status: None });
            None
        }
    }
}

async fn fetch_metrics(client: &Client) -> EndpointInfo {
    match client.get("/metrics").await {
        Ok(_) => EndpointInfo { status: "ok".into(), http_status: Some(200) },
        Err(Error::Http { status, .. }) if matches!(status, 401 | 403) => {
            match client.get_unauthenticated("/metrics").await {
                Ok(_) => EndpointInfo { status: "ok".into(), http_status: Some(200) },
                Err(Error::Http { status, .. }) => EndpointInfo { status: endpoint_status(status).into(), http_status: Some(status) },
                Err(_) => EndpointInfo { status: "unreachable".into(), http_status: None },
            }
        }
        Err(Error::Http { status, .. }) => EndpointInfo { status: endpoint_status(status).into(), http_status: Some(status) },
        Err(_) => EndpointInfo { status: "unreachable".into(), http_status: None },
    }
}
```

- [ ] **Step 3: Build**

Run: `cargo build -p llminate-core`
Expected: compiles (no warnings).

- [ ] **Step 4: Commit**

```bash
git add crates/llminate-core/src/client.rs crates/llminate-core/src/probe.rs
git commit -m "feat(core): probe() orchestrator + Client::has_api_key"
```

## Task 3: CLI `probe` (non-interactive + `--json`)

**Files:**
- Modify: `crates/llminate/src/cli.rs`
- Modify: `crates/llminate/src/main.rs`
- Modify: `crates/llminate/src/commands.rs`
- Create: `crates/llminate/src/commands/probe.rs`

- [ ] **Step 1: Add the `Probe` subcommand to `cli.rs`**

Inside `pub enum Command { … }`, add:

```rust
    /// Inspect an OpenAI-compatible endpoint (onboarding probe)
    Probe {
        /// Emit one JSON document (machine contract)
        #[arg(long)]
        json: bool,
        /// Force the interactive wizard (requires a TTY)
        #[arg(long, short = 'i')]
        interactive: bool,
        /// Persist the probed endpoint as a target on success
        #[arg(long = "save-as", value_name = "NAME")]
        save_as: Option<String>,
        /// With --save-as, also set default_target
        #[arg(long = "set-default")]
        set_default: bool,
    },
```

- [ ] **Step 2: Wire it in `main.rs`**

In `run()`, handle `Probe` before the generic dispatch (it needs its own exit code):

```rust
async fn run() -> i32 {
    let cli = Cli::parse();
    let style = Style::new(cli.no_color);
    if let Command::Probe { .. } = cli.command {
        return match commands::probe::run(&cli, style).await {
            Ok(code) => code,
            Err(e) => {
                eprintln!("llminate: {e}");
                e.exit_code()
            }
        };
    }
    match dispatch(cli, style).await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("llminate: {e}");
            e.exit_code()
        }
    }
}
```

And add `Command::Probe { .. }` to the existing `unreachable!()` arm of `dispatch`.

- [ ] **Step 3: Declare the module in `commands.rs`**

At the top of `crates/llminate/src/commands.rs`, add:

```rust
pub mod probe;
```

- [ ] **Step 4: Create `crates/llminate/src/commands/probe.rs`**

```rust
//! `llminate probe` — endpoint onboarding probe.

use crate::cli::Cli;
use crate::output::Style;
use llminate_core::config::{self, Overrides};
use llminate_core::probe::{probe, ProbeReport};
use llminate_core::{Client, Error, Result};

pub async fn run(cli: &Cli, style: Style) -> Result<i32> {
    let (json, _interactive, _save_as, _set_default) = flags(cli);
    let cfg = config::load()?;
    let ov = Overrides {
        target: cli.target.clone(),
        url: cli.url.clone(),
        api_key: cli.api_key.clone(),
        model: cli.model.clone(),
        timeout_secs: cli.timeout,
    };
    let target = config::resolve(&cfg, &ov)?;
    let client = Client::new(&target)?;
    let report = probe(&client, target.name.clone()).await;

    if json {
        let doc = serde_json::to_string_pretty(&report).map_err(|e| Error::msg(e.to_string()))?;
        println!("{doc}");
    } else {
        render_human(&report, &style);
    }
    Ok(exit_code(&report))
}

fn flags(cli: &Cli) -> (bool, bool, Option<String>, bool) {
    match &cli.command {
        crate::cli::Command::Probe {
            json,
            interactive,
            save_as,
            set_default,
        } => (*json, *interactive, save_as.clone(), *set_default),
        _ => unreachable!("run() is only called for Probe"),
    }
}

fn exit_code(r: &ProbeReport) -> i32 {
    let models_ok = r
        .endpoints
        .get("/v1/models")
        .map(|e| e.status == "ok")
        .unwrap_or(false);
    if models_ok {
        0
    } else {
        1
    }
}

fn render_human(r: &ProbeReport, _s: &Style) {
    println!("probe {}", r.url);
    println!("  auth:      {}", r.auth.status);
    let ver = r
        .engine
        .version
        .as_deref()
        .map(|v| format!(" {v}"))
        .unwrap_or_default();
    println!("  engine:    {}{}", r.engine.kind, ver);
    if r.models.is_empty() {
        println!("  models:    (aucun)");
    } else {
        for (i, m) in r.models.iter().enumerate() {
            let label = if i == 0 { "models:   " } else { "          " };
            println!("  {label} {}", m.id);
        }
    }
    println!("  health:    {}", r.health.status);
    println!("  latency:   {}ms", r.latency_ms);
    let eps: Vec<String> = r
        .endpoints
        .iter()
        .map(|(p, e)| format!("{p} {}", e.status))
        .collect();
    println!("  endpoints: {}", eps.join(" · "));
    println!("=> {}", if r.ok { "OK" } else { "FAILED" });
}
```

- [ ] **Step 5: Build + manual smoke**

Run: `cargo build -p llminate`
Run: `./target/debug/llminate probe -t spark`
Expected: a human report ending in `=> OK`, exit 0 (`echo $?`).
Run: `./target/debug/llminate probe -t spark --json | python3 -m json.tool | head`
Expected: valid JSON with `"probe_version": 1`.

- [ ] **Step 6: Commit**

```bash
git add crates/llminate/src/cli.rs crates/llminate/src/main.rs crates/llminate/src/commands.rs crates/llminate/src/commands/probe.rs
git commit -m "feat(cli): llminate probe (human + --json)"
```

## Task 4: interactive wizard (`dialoguer`) + `--save-as`

**Files:**
- Modify: `Cargo.toml` (workspace)
- Modify: `crates/llminate/Cargo.toml`
- Modify: `crates/llminate-core/src/config.rs`
- Modify: `crates/llminate/src/commands/probe.rs`

- [ ] **Step 1: Add the dependency**

Workspace `Cargo.toml`, in `[workspace.dependencies]`:
```toml
dialoguer = "0.11"
```
`crates/llminate/Cargo.toml`, in `[dependencies]`:
```toml
dialoguer.workspace = true
```

- [ ] **Step 2: Add `config::add_target` (`llminate-core/src/config.rs`)**

```rust
/// Append a `[targets.<name>]` block (url only) to the config file.
/// Errors if the target already exists. Does not store any secret.
pub fn add_target(path: &Path, name: &str, url: &str) -> Result<()> {
    let cfg = load_from(path)?;
    if cfg.targets.contains_key(name) {
        return Err(Error::Config(format!("cible '{name}' existe déjà")));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = std::fs::read_to_string(path).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(&format!("[targets.{name}]\nurl = \"{url}\"\n"));
    std::fs::write(path, text)?;
    Ok(())
}
```

- [ ] **Step 3: Add the wizard to `commands/probe.rs`**

Add imports:
```rust
use llminate_core::ResolvedTarget;
use std::io::IsTerminal;
```
Replace the body of `run` with the mode selection, and add `wizard`, `adhoc`, `save_target`:

```rust
pub async fn run(cli: &Cli, style: Style) -> Result<i32> {
    let (json, interactive, save_as, set_default) = flags(cli);
    let tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let has_selector = cli.target.is_some() || cli.url.is_some();

    if interactive || (!json && tty && !has_selector) {
        return wizard(save_as, set_default, style).await;
    }

    let cfg = config::load()?;
    let ov = Overrides {
        target: cli.target.clone(),
        url: cli.url.clone(),
        api_key: cli.api_key.clone(),
        model: cli.model.clone(),
        timeout_secs: cli.timeout,
    };
    let target = config::resolve(&cfg, &ov)?;
    let client = Client::new(&target)?;
    let report = probe(&client, target.name.clone()).await;

    if json {
        let doc = serde_json::to_string_pretty(&report).map_err(|e| Error::msg(e.to_string()))?;
        println!("{doc}");
    } else {
        render_human(&report, &style);
    }
    if let Some(name) = &save_as {
        if report.ok {
            let url = cli.url.clone().unwrap_or_else(|| target.url.clone());
            save_target(&url, name, set_default)?;
            eprintln!("✓ cible « {name} » ajoutée");
        }
    }
    Ok(exit_code(&report))
}

fn adhoc(url: &str, key: &str) -> Result<ResolvedTarget> {
    Ok(ResolvedTarget {
        name: None,
        url: url.to_string(),
        model: None,
        api_key: if key.is_empty() { None } else { Some(key.to_string()) },
        timeout: ResolvedTarget::DEFAULT_TIMEOUT,
    })
}

async fn wizard(save_as: Option<String>, set_default: bool, style: Style) -> Result<i32> {
    use dialoguer::{Confirm, Input, Password};
    let ok = |e: dialoguer::Error| Error::msg(e.to_string());
    loop {
        let url: String = Input::new()
            .with_prompt("URL de l'endpoint")
            .default("http://192.168.1.87:30000".to_string())
            .interact_text()
            .map_err(ok)?;
        let key: String = Password::new()
            .with_prompt("Clé API (vide si non requise)")
            .allow_empty_password(true)
            .interact()
            .map_err(ok)?;

        let target = adhoc(&url, &key)?;
        let client = Client::new(&target)?;
        println!("Analyse…");
        let report = probe(&client, None).await;
        render_human(&report, &style);

        if report.ok {
            let want = Confirm::new()
                .with_prompt("Enregistrer comme cible llminate ?")
                .default(false)
                .interact()
                .unwrap_or(false);
            if want {
                let name = match save_as.clone() {
                    Some(n) => n,
                    None => Input::new()
                        .with_prompt("Nom de la cible")
                        .interact_text()
                        .map_err(ok)?,
                };
                save_target(&url, &name, set_default)?;
                println!("✓ cible « {name} » ajoutée (clé non stockée — configure api_key_env ou api_key_bws)");
            }
            return Ok(exit_code(&report));
        }

        let retry = Confirm::new()
            .with_prompt("Réessayer ?")
            .default(true)
            .interact()
            .unwrap_or(false);
        if !retry {
            return Ok(exit_code(&report));
        }
    }
}

fn save_target(url: &str, name: &str, set_default: bool) -> Result<()> {
    let path = config::config_path()
        .ok_or_else(|| Error::Config("chemin de config introuvable (HOME ?)".into()))?;
    config::add_target(&path, name, url)?;
    if set_default {
        config::set_default_target(&path, name)?;
    }
    Ok(())
}
```

- [ ] **Step 4: Build**

Run: `cargo build -p llminate`
Expected: compiles.

- [ ] **Step 5: Manual wizard check (TTY)**

Run: `./target/debug/llminate probe`
Expected: prompts URL/key, shows the report, offers to save.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/llminate/Cargo.toml crates/llminate-core/src/config.rs crates/llminate/src/commands/probe.rs
git commit -m "feat(cli): interactive probe wizard (dialoguer) + --save-as"
```

## Task 5: integration tests

**Files:**
- Modify: `crates/llminate/tests/integration.rs`

- [ ] **Step 1: Extract a `start(app)` helper and reuse it in `base()`**

Replace the `base()` function with:

```rust
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
    let port = rx.recv().expect("recv port");
    format!("http://127.0.0.1:{port}")
}

fn base() -> &'static str {
    static BASE: OnceLock<String> = OnceLock::new();
    BASE.get_or_init(|| start(app()))
}
```

- [ ] **Step 2: Add an auth-requiring stub**

```rust
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
        .route("/metrics", get(metrics))
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
```

- [ ] **Step 3: Add the probe tests**

```rust
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
    assert_eq!(o.status.code(), Some(1));
    let v: Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(v["auth"]["status"], "rejected");
    assert_eq!(v["ok"], false);
}

#[test]
fn probe_auth_required_missing() {
    let o = run_at(&["probe", "--json"], base_auth(), None);
    assert_eq!(o.status.code(), Some(1));
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
```

- [ ] **Step 4: Run the full suite**

Run: `cargo test`
Expected: all tests pass (existing + 5 new probe tests).
Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: clean.
Run: `cargo fmt --all`

- [ ] **Step 5: Commit**

```bash
git add crates/llminate/tests/integration.rs
git commit -m "test: probe contract, auth paths, --save-as"
```

## Task 6: docs + version bump 0.3.0

**Files:**
- Modify: `README.md`
- Modify: `docs/design.md`
- Modify: `Cargo.toml` (workspace)
- Modify: `docs/specs/probe.md` (status)

- [ ] **Step 1: README — add `probe` to the commands table**

In the commands table, add a row:

```markdown
| `probe` | `/v1/models`, `/health`, `/version`, `/metrics` | endpoint onboarding: JSON report or interactive wizard (`--json`, `--save-as`) |
```

- [ ] **Step 2: README — add an "Endpoint onboarding" section**

Add after the "Commands" table:

````markdown
## Endpoint onboarding (`probe`)

Validate a new OpenAI-compatible endpoint and inventory it. For backends (any
language), `--json` emits one stable document:

```
LLMINATE_API_KEY=… llminate probe --json --url http://host:30000
```

```json
{ "probe_version": 1, "ok": true,
  "auth": {"status": "ok", "http_status": 200},
  "engine": {"kind": "vllm", "version": "0.32"},
  "models": [{"id": "…", "owned_by": "vllm"}],
  "endpoints": {"/v1/models": {"status": "ok", "http_status": 200}, "…": {}} }
```

- Exit `0` iff `/v1/models` responds 2xx; `1` otherwise; `2` usage/config.
- `ok` = auth OK **and** at least one model.
- `auth.status`: `ok` · `not_required` · `rejected` · `required_missing`.
- `engine.kind`: `vllm` · `sglang` · `openai-compatible` · `unknown`.
- With no arguments on a TTY, an interactive wizard guides you; `--save-as NAME`
  persists the endpoint as a target (key not stored — set `api_key_env`/`api_key_bws`).
````

- [ ] **Step 3: `docs/design.md` — add a probe bullet**

Add under "Commands":

```markdown
- `probe [--json|--interactive] [--save-as NAME]` — endpoint onboarding: auth,
  engine, models, endpoint availability, health/latency. `ProbeReport` lives in
  `llminate-core::probe` for embedding; the CLI adds rendering + the wizard.
```

- [ ] **Step 4: Bump the version to 0.3.0**

In the workspace `Cargo.toml`:
- `version = "0.3.0"`
- `llminate-core = { version = "0.3.0", path = "crates/llminate-core" }`

- [ ] **Step 5: Mark the spec implemented**

In `docs/specs/probe.md`, change `**Status:** proposed` to `**Status:** implemented (v0.3.0)`.

- [ ] **Step 6: final checks + commit**

Run: `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test`
Expected: clean, all pass.

```bash
git add README.md docs/design.md docs/specs/probe.md Cargo.toml Cargo.lock
git commit -m "docs: document probe; bump to 0.3.0"
```

- [ ] **Step 7: Release (only when asked)**

```bash
git push origin main
git tag -a v0.3.0 -m "llminate v0.3.0"
git push origin v0.3.0
```

---

## Self-review

**Spec coverage:** command surface + flags → Tasks 3/4; JSON contract → Tasks 1/2/5;
auth/engine/endpoint vocabularies → Task 1; exit codes → Task 3; failure matrix → Task 5;
wizard + save → Task 4; core placement → Tasks 1/2; tests → Tasks 1/5; docs → Task 6.
All covered.

**Known deviation:** progress lines are rendered from the completed report rather than
streamed live. Probes finish in ~200 ms, so this is not perceptible; noted for v2.
