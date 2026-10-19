//! Multi-target configuration: load `config.toml`, resolve the effective target.
//!
//! Precedence, highest first:
//! 1. CLI flags (`Overrides`)
//! 2. `LLMINATE_*` environment variables
//! 3. the chosen `[targets.<name>]` block
//! 4. built-in defaults

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Root of the on-disk configuration.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_target: Option<String>,
    #[serde(default)]
    pub targets: BTreeMap<String, Target>,
}

/// A single named endpoint (one GB10, one vLLM/SGLang server, ...).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Name of the environment variable holding the API key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// Literal API key. Discouraged: keep secrets out of the config file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Shell command whose stdout is the API key (generic secret injection,
    /// e.g. `pass`, `op`, a wrapper around `bws`...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_cmd: Option<String>,
    /// Bitwarden Secrets Manager key name, fetched with `bws secret list`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_bws: Option<String>,
    /// BWS project id for `api_key_bws` (default: `$BWS_PROJECT_ID`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bws_project: Option<String>,
    /// BWS server URL for `api_key_bws` (default: `$BWS_SERVER_URL`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bws_server: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// Values coming from the command line, overriding everything else.
#[derive(Debug, Default, Clone)]
pub struct Overrides {
    pub target: Option<String>,
    pub url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub timeout_secs: Option<u64>,
}

/// The fully-resolved connection settings for one invocation.
#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    pub name: Option<String>,
    pub url: String,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub timeout: u64,
}

impl ResolvedTarget {
    /// Default request timeout, in seconds.
    pub const DEFAULT_TIMEOUT: u64 = 30;

    /// Return the API key or a usage error (exit code 2).
    pub fn require_api_key(&self) -> Result<&str> {
        match self.api_key.as_deref() {
            Some(k) if !k.is_empty() => Ok(k),
            _ => Err(Error::MissingApiKey),
        }
    }

    /// Human-readable target name, for messages.
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or("(sans nom)")
    }
}

/// Where an API key comes from, in precedence order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKeySource {
    /// A literal value (CLI flag or `api_key` field).
    Literal(String),
    /// Read from an environment variable of that name.
    Env(String),
    /// Run a shell command, take its trimmed stdout.
    Cmd(String),
    /// Fetch from Bitwarden Secrets Manager via `bws`.
    Bws {
        key: String,
        project: Option<String>,
        server: Option<String>,
    },
}

/// Ordered key candidates (highest precedence first), without resolving them.
pub fn key_sources(ov: &Overrides, target: Option<&Target>) -> Vec<ApiKeySource> {
    let mut v = Vec::new();
    if let Some(k) = &ov.api_key {
        v.push(ApiKeySource::Literal(k.clone()));
    }
    v.push(ApiKeySource::Env("LLMINATE_API_KEY".to_string()));
    if let Some(t) = target {
        if let Some(name) = &t.api_key_bws {
            v.push(ApiKeySource::Bws {
                key: name.clone(),
                project: t.bws_project.clone(),
                server: t.bws_server.clone(),
            });
        }
        if let Some(cmd) = &t.api_key_cmd {
            v.push(ApiKeySource::Cmd(cmd.clone()));
        }
        if let Some(e) = &t.api_key_env {
            v.push(ApiKeySource::Env(e.clone()));
        }
        if let Some(k) = &t.api_key {
            v.push(ApiKeySource::Literal(k.clone()));
        }
    }
    v
}

/// Walk the candidates and return the first non-empty key.
fn resolve_key(
    ov: &Overrides,
    target: Option<&Target>,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<Option<String>> {
    for src in key_sources(ov, target) {
        if let Some(k) = materialize_key(&src, env)? {
            return Ok(Some(k));
        }
    }
    Ok(None)
}

/// Resolve one source into a concrete key, or `Ok(None)` if it is merely absent.
pub fn materialize_key(
    source: &ApiKeySource,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<Option<String>> {
    match source {
        ApiKeySource::Literal(s) => Ok(some_nonempty(s.clone())),
        ApiKeySource::Env(name) => Ok(env(name).and_then(some_nonempty)),
        ApiKeySource::Cmd(cmd) => run_sh(cmd),
        ApiKeySource::Bws {
            key,
            project,
            server,
        } => fetch_bws(key, project, server, env).map(Some),
    }
}

fn some_nonempty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn run_sh(cmd: &str) -> Result<Option<String>> {
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .map_err(|e| Error::Config(format!("api_key_cmd: impossible de lancer `sh` ({e})")))?;
    if !out.status.success() {
        let code = out
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".to_string());
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(Error::Config(format!(
            "api_key_cmd a échoué (code {code}): {}",
            err.trim()
        )));
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(some_nonempty(s))
}

fn fetch_bws(
    key: &str,
    project: &Option<String>,
    server: &Option<String>,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<String> {
    let project = project.clone().or_else(|| env("BWS_PROJECT_ID"));
    let server = server.clone().or_else(|| env("BWS_SERVER_URL"));
    let mut cmd = std::process::Command::new("bws");
    cmd.args(["secret", "list"]);
    if let Some(p) = &project {
        cmd.arg(p);
    }
    cmd.args(["--output", "json"]);
    if let Some(s) = &server {
        cmd.args(["--server-url", s]);
    }
    let out = cmd
        .output()
        .map_err(|e| Error::Config(format!("api_key_bws: `bws` introuvable ({e})")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(Error::Config(format!(
            "api_key_bws: `bws secret list` a échoué: {}",
            err.trim()
        )));
    }
    parse_bws(&String::from_utf8_lossy(&out.stdout), key)
}

/// Pick `key` from a `bws secret list --output json` payload.
pub fn parse_bws(json: &str, key: &str) -> Result<String> {
    let v: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| Error::Config(format!("api_key_bws: JSON bws invalide ({e})")))?;
    let val = v
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| s["key"].as_str() == Some(key))
        .and_then(|s| s["value"].as_str())
        .unwrap_or("");
    if val.is_empty() {
        return Err(Error::Config(format!(
            "api_key_bws: secret '{key}' introuvable dans Bitwarden"
        )));
    }
    Ok(val.to_string())
}

/// Human label for a target's configured key source (never runs it).
pub fn key_source_label(t: &Target) -> String {
    if let Some(k) = &t.api_key_bws {
        format!("bws:{k}")
    } else if t.api_key_cmd.is_some() {
        "cmd".to_string()
    } else if let Some(e) = &t.api_key_env {
        format!("env:{e}")
    } else if t.api_key.is_some() {
        "littéral".to_string()
    } else {
        "—".to_string()
    }
}

/// Path to the config file: `$LLMINATE_CONFIG`, else `$XDG_CONFIG_HOME/llminate/config.toml`,
/// else `~/.config/llminate/config.toml`.
pub fn config_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("LLMINATE_CONFIG") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("llminate").join("config.toml"))
}

/// Load the config from its default location (missing file = empty config).
pub fn load() -> Result<Config> {
    match config_path() {
        Some(p) => load_from(&p),
        None => Ok(Config::default()),
    }
}

/// Load the config from an explicit path (missing file = empty config).
pub fn load_from(path: &Path) -> Result<Config> {
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = std::fs::read_to_string(path)?;
    toml::from_str(&text).map_err(|e| Error::Config(format!("{}: {e}", path.display())))
}

/// Parse config from a string (used by tests).
pub fn parse(text: &str) -> Result<Config> {
    toml::from_str(text).map_err(|e| Error::Config(e.to_string()))
}

/// Resolve the effective target using the process environment.
pub fn resolve(cfg: &Config, ov: &Overrides) -> Result<ResolvedTarget> {
    resolve_with(cfg, ov, &|k| std::env::var(k).ok())
}

/// Resolve the effective target against an injected environment lookup.
pub fn resolve_with(
    cfg: &Config,
    ov: &Overrides,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<ResolvedTarget> {
    let chosen = ov
        .target
        .clone()
        .or_else(|| env("LLMINATE_TARGET"))
        .or_else(|| cfg.default_target.clone());
    let url_override = ov.url.clone().or_else(|| env("LLMINATE_BASE_URL"));

    let (name, target): (Option<String>, Option<&Target>) = match &chosen {
        Some(n) => {
            let t = cfg.targets.get(n).ok_or_else(|| {
                Error::Config(format!("cible inconnue '{n}' (connues: {})", known(cfg)))
            })?;
            (Some(n.clone()), Some(t))
        }
        None => {
            if url_override.is_some() {
                (None, None)
            } else if cfg.targets.len() == 1 {
                let (n, t) = cfg.targets.iter().next().expect("len == 1");
                (Some(n.clone()), Some(t))
            } else if cfg.targets.is_empty() {
                return Err(Error::Config(
                    "aucune cible définie — exporte LLMINATE_BASE_URL ou crée [targets.<nom>] dans la config"
                        .into(),
                ));
            } else {
                return Err(Error::Config(format!(
                    "plusieurs cibles disponibles ({}) — précise -t/--target <nom>",
                    known(cfg)
                )));
            }
        }
    };

    let url = ov
        .url
        .clone()
        .or_else(|| env("LLMINATE_BASE_URL"))
        .or_else(|| target.map(|t| t.url.clone()))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            Error::Config(
                "URL de base manquante (--url, LLMINATE_BASE_URL, ou [targets.<nom>].url)".into(),
            )
        })?;

    let model = ov
        .model
        .clone()
        .or_else(|| env("LLMINATE_MODEL"))
        .or_else(|| target.and_then(|t| t.model.clone()))
        .filter(|s| !s.is_empty());

    let api_key = resolve_key(ov, target, env)?;

    let timeout = ov
        .timeout_secs
        .or_else(|| env("LLMINATE_TIMEOUT").and_then(|s| s.parse::<u64>().ok()))
        .or_else(|| target.and_then(|t| t.timeout_secs))
        .filter(|t| *t > 0)
        .unwrap_or(ResolvedTarget::DEFAULT_TIMEOUT);

    Ok(ResolvedTarget {
        name,
        url,
        model,
        api_key,
        timeout,
    })
}

/// Set `default_target` in the config file, preserving the rest of the text.
pub fn set_default_target(path: &Path, name: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let line = format!("default_target = \"{name}\"");
    let mut out = String::new();
    let mut replaced = false;
    for l in existing.lines() {
        let trimmed = l.trim_start();
        if trimmed.starts_with("default_target") && trimmed.contains('=') {
            out.push_str(&line);
            out.push('\n');
            replaced = true;
        } else {
            out.push_str(l);
            out.push('\n');
        }
    }
    if !replaced {
        let mut with = format!("{line}\n");
        if !out.is_empty() {
            with.push('\n');
            with.push_str(&out);
        }
        out = with;
    }
    std::fs::write(path, out)?;
    Ok(())
}

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

fn known(cfg: &Config) -> String {
    if cfg.targets.is_empty() {
        "(aucune)".to_string()
    } else {
        cfg.targets.keys().cloned().collect::<Vec<_>>().join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k: &str| {
            pairs
                .iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| v.to_string())
        }
    }

    const SAMPLE: &str = r#"
default_target = "spark"

[targets.spark]
url = "http://192.168.1.87:30000"
model = "qwen3.8-flash-next"
api_key_env = "SPARK_KEY"

[targets.lab]
url = "http://100.70.22.24:30000"
timeout_secs = 5
"#;

    #[test]
    fn parses_targets() {
        let cfg = parse(SAMPLE).unwrap();
        assert_eq!(cfg.default_target.as_deref(), Some("spark"));
        assert_eq!(cfg.targets.len(), 2);
        assert_eq!(cfg.targets["spark"].url, "http://192.168.1.87:30000");
        assert_eq!(cfg.targets["lab"].timeout_secs, Some(5));
    }

    #[test]
    fn default_target_used_and_api_key_env_resolved() {
        let cfg = parse(SAMPLE).unwrap();
        let t = resolve_with(
            &cfg,
            &Overrides::default(),
            &env_of(&[("SPARK_KEY", "sk-x")]),
        )
        .unwrap();
        assert_eq!(t.name.as_deref(), Some("spark"));
        assert_eq!(t.url, "http://192.168.1.87:30000");
        assert_eq!(t.model.as_deref(), Some("qwen3.8-flash-next"));
        assert_eq!(t.api_key.as_deref(), Some("sk-x"));
        assert_eq!(t.timeout, 30);
    }

    #[test]
    fn explicit_target_beats_default() {
        let cfg = parse(SAMPLE).unwrap();
        let ov = Overrides {
            target: Some("lab".into()),
            ..Default::default()
        };
        let t = resolve_with(&cfg, &ov, &env_of(&[])).unwrap();
        assert_eq!(t.name.as_deref(), Some("lab"));
        assert_eq!(t.timeout, 5);
    }

    #[test]
    fn env_target_beats_default() {
        let cfg = parse(SAMPLE).unwrap();
        let t = resolve_with(
            &cfg,
            &Overrides::default(),
            &env_of(&[("LLMINATE_TARGET", "lab")]),
        )
        .unwrap();
        assert_eq!(t.name.as_deref(), Some("lab"));
    }

    #[test]
    fn cli_overrides_beat_env_and_config() {
        let cfg = parse(SAMPLE).unwrap();
        let ov = Overrides {
            url: Some("http://cli:1".into()),
            timeout_secs: Some(99),
            ..Default::default()
        };
        let t = resolve_with(
            &cfg,
            &ov,
            &env_of(&[
                ("LLMINATE_BASE_URL", "http://env:2"),
                ("LLMINATE_TIMEOUT", "42"),
            ]),
        )
        .unwrap();
        assert_eq!(t.url, "http://cli:1");
        assert_eq!(t.timeout, 99);
    }

    #[test]
    fn unknown_target_is_config_error() {
        let cfg = parse(SAMPLE).unwrap();
        let ov = Overrides {
            target: Some("nope".into()),
            ..Default::default()
        };
        let err = resolve_with(&cfg, &ov, &env_of(&[])).unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn multiple_targets_without_selection_fails() {
        let cfg = parse(SAMPLE).unwrap();
        let cfg = Config {
            default_target: None,
            ..cfg
        };
        let err = resolve_with(&cfg, &Overrides::default(), &env_of(&[])).unwrap_err();
        assert!(err.to_string().contains("plusieurs cibles"));
    }

    #[test]
    fn single_target_autoselected() {
        let cfg = parse("[targets.only]\nurl = \"http://x:1\"\n").unwrap();
        let t = resolve_with(&cfg, &Overrides::default(), &env_of(&[])).unwrap();
        assert_eq!(t.name.as_deref(), Some("only"));
    }

    #[test]
    fn url_override_without_config_target() {
        let cfg = Config::default();
        let ov = Overrides {
            url: Some("http://anon:1".into()),
            ..Default::default()
        };
        let t = resolve_with(&cfg, &ov, &env_of(&[])).unwrap();
        assert!(t.name.is_none());
        assert_eq!(t.url, "http://anon:1");
    }

    #[test]
    fn no_config_no_url_fails_clean() {
        let err =
            resolve_with(&Config::default(), &Overrides::default(), &env_of(&[])).unwrap_err();
        assert!(err.to_string().contains("aucune cible"));
    }

    #[test]
    fn literal_api_key_in_config_works() {
        let cfg = parse("[targets.a]\nurl = \"http://x:1\"\napi_key = \"sk-literal\"\n").unwrap();
        let t = resolve_with(&cfg, &Overrides::default(), &env_of(&[])).unwrap();
        assert_eq!(t.api_key.as_deref(), Some("sk-literal"));
    }

    #[test]
    fn set_default_target_preserves_body() {
        let dir = std::env::temp_dir().join(format!("llminate-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("config.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, SAMPLE).unwrap();

        set_default_target(&path, "lab").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("default_target = \"lab\""));
        assert!(!text.contains("\"spark\"\n\n[targets.spark]")); // replaced, not duplicated
        assert!(text.contains("[targets.spark]"));
        let reparsed = load_from(&path).unwrap();
        assert_eq!(reparsed.default_target.as_deref(), Some("lab"));

        // second call on a file without the key must prepend it
        let path2 = dir.join("fresh.toml");
        std::fs::write(&path2, "[targets.a]\nurl=\"http://x:1\"\n").unwrap();
        set_default_target(&path2, "a").unwrap();
        let text2 = std::fs::read_to_string(&path2).unwrap();
        assert!(text2.starts_with("default_target = \"a\"\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn key_sources_precedence_order() {
        let cfg = parse(
            "[targets.a]\nurl = \"http://x:1\"\napi_key_bws = \"K\"\napi_key_cmd = \"echo c\"\napi_key_env = \"E\"\napi_key = \"lit\"\n",
        )
        .unwrap();
        let t = &cfg.targets["a"];
        let ov = Overrides {
            api_key: Some("cli".into()),
            ..Default::default()
        };
        let srcs = key_sources(&ov, Some(t));
        assert_eq!(
            srcs,
            vec![
                ApiKeySource::Literal("cli".into()),
                ApiKeySource::Env("LLMINATE_API_KEY".into()),
                ApiKeySource::Bws {
                    key: "K".into(),
                    project: None,
                    server: None
                },
                ApiKeySource::Cmd("echo c".into()),
                ApiKeySource::Env("E".into()),
                ApiKeySource::Literal("lit".into()),
            ]
        );
    }

    #[test]
    fn cli_key_beats_bws() {
        let cfg = parse("[targets.a]\nurl=\"http://x:1\"\napi_key_bws=\"K\"\n").unwrap();
        let ov = Overrides {
            api_key: Some("cli".into()),
            ..Default::default()
        };
        let t = resolve_with(&cfg, &ov, &env_of(&[])).unwrap();
        assert_eq!(t.api_key.as_deref(), Some("cli"));
    }

    #[test]
    fn env_beats_bws_without_running_bws() {
        let cfg = parse("[targets.a]\nurl=\"http://x:1\"\napi_key_bws=\"K\"\n").unwrap();
        let t = resolve_with(
            &cfg,
            &Overrides::default(),
            &env_of(&[("LLMINATE_API_KEY", "sk-env")]),
        )
        .unwrap();
        assert_eq!(t.api_key.as_deref(), Some("sk-env"));
    }

    #[test]
    fn api_key_cmd_stdout_is_trimmed() {
        let src = ApiKeySource::Cmd("echo secret-value".into());
        let got = materialize_key(&src, &env_of(&[])).unwrap();
        assert_eq!(got.as_deref(), Some("secret-value"));
    }

    #[test]
    fn api_key_cmd_failure_is_a_config_error() {
        let src = ApiKeySource::Cmd("exit 3".into());
        let err = materialize_key(&src, &env_of(&[])).unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn api_key_cmd_empty_falls_through_to_next() {
        let cfg = parse("[targets.a]\nurl=\"http://x:1\"\napi_key_cmd=\"true\"\napi_key=\"lit\"\n")
            .unwrap();
        let t = resolve_with(&cfg, &Overrides::default(), &env_of(&[])).unwrap();
        assert_eq!(t.api_key.as_deref(), Some("lit"));
    }

    #[test]
    fn parse_bws_picks_the_key() {
        let json = r#"[{"key":"OTHER","value":"x"},{"key":"VLLM_API_KEY","value":"sk-bws"}]"#;
        assert_eq!(parse_bws(json, "VLLM_API_KEY").unwrap(), "sk-bws");
        assert!(parse_bws(json, "MISSING").is_err());
        assert!(parse_bws("not json", "K").is_err());
    }

    #[test]
    fn key_source_label_reports_kind() {
        let cfg = parse("[targets.a]\nurl=\"http://x:1\"\napi_key_bws=\"VLLM_API_KEY\"\n").unwrap();
        assert_eq!(key_source_label(&cfg.targets["a"]), "bws:VLLM_API_KEY");
        let cfg2 = parse("[targets.a]\nurl=\"http://x:1\"\n").unwrap();
        assert_eq!(key_source_label(&cfg2.targets["a"]), "—");
    }
}
