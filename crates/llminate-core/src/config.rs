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
    /// Name of the environment variable holding the API key (preferred).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// Literal API key. Discouraged: keep secrets in the environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
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

    let api_key = ov
        .api_key
        .clone()
        .or_else(|| env("LLMINATE_API_KEY"))
        .or_else(|| {
            target
                .and_then(|t| t.api_key_env.as_ref())
                .and_then(|k| env(k))
        })
        .or_else(|| target.and_then(|t| t.api_key.clone()))
        .filter(|s| !s.is_empty());

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
}
