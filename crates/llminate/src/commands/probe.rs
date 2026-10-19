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
