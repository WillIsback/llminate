//! `llminate probe` — endpoint onboarding probe.

use crate::cli::Cli;
use crate::output::Style;
use llminate_core::config::{self, Overrides};
use llminate_core::probe::{probe, ProbeReport};
use llminate_core::{Client, Error, ResolvedTarget, Result};
use std::io::IsTerminal;

struct ProbeArgs {
    json: bool,
    interactive: bool,
    save_as: Option<String>,
    set_default: bool,
}

fn args(cli: &Cli) -> ProbeArgs {
    match &cli.command {
        crate::cli::Command::Probe {
            json,
            interactive,
            save_as,
            set_default,
        } => ProbeArgs {
            json: *json,
            interactive: *interactive,
            save_as: save_as.clone(),
            set_default: *set_default,
        },
        _ => unreachable!("args() is only called for Probe"),
    }
}

pub async fn run(cli: &Cli, style: Style) -> Result<i32> {
    let a = args(cli);
    let tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let has_selector = cli.target.is_some() || cli.url.is_some();

    if a.interactive || (!a.json && tty && !has_selector) {
        return wizard(a.save_as, a.set_default, style).await;
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

    if a.json {
        let doc = serde_json::to_string_pretty(&report).map_err(|e| Error::msg(e.to_string()))?;
        println!("{doc}");
    } else {
        render_human(&report, &style);
    }
    if let Some(name) = &a.save_as {
        if report.ok {
            let url = cli.url.clone().unwrap_or_else(|| target.url.clone());
            save_target(&url, name, a.set_default)?;
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
        api_key: if key.is_empty() {
            None
        } else {
            Some(key.to_string())
        },
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

fn exit_code(r: &ProbeReport) -> i32 {
    exit_code_from(r.endpoints.get("/v1/models").map(|e| e.status.as_str()))
}

fn exit_code_from(models_status: Option<&str>) -> i32 {
    if models_status == Some("ok") {
        0
    } else {
        1
    }
}

fn render_human(r: &ProbeReport, s: &Style) {
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
    println!("=> {}", s.bold(if r.ok { "OK" } else { "FAILED" }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_maps_models_status() {
        assert_eq!(exit_code_from(Some("ok")), 0);
        assert_eq!(exit_code_from(Some("denied")), 1);
        assert_eq!(exit_code_from(Some("unreachable")), 1);
        assert_eq!(exit_code_from(None), 1);
    }
}
