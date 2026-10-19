//! Command-line surface, defined with clap derive.

use clap::{Parser, Subcommand};
use clap_complete::Shell;

/// Top-level CLI.
#[derive(Debug, Parser)]
#[command(
    name = "llminate",
    version,
    about = "to LLMinate — probe and test any OpenAI-compatible LLM server (vLLM, SGLang)"
)]
pub struct Cli {
    /// Cible nommée (bloc [targets.<nom>])
    #[arg(short = 't', long, global = true, env = "LLMINATE_TARGET")]
    pub target: Option<String>,

    /// URL de base (override)
    #[arg(long, global = true, env = "LLMINATE_BASE_URL", value_name = "URL")]
    pub url: Option<String>,

    /// Clé API (override ; préférer LLMINATE_API_KEY)
    #[arg(
        long = "api-key",
        global = true,
        env = "LLMINATE_API_KEY",
        hide_env_values = true
    )]
    pub api_key: Option<String>,

    /// Modèle par défaut (sinon premier id de /v1/models)
    #[arg(long, global = true, env = "LLMINATE_MODEL")]
    pub model: Option<String>,

    /// Timeout en secondes
    #[arg(long, global = true, env = "LLMINATE_TIMEOUT")]
    pub timeout: Option<u64>,

    /// Désactive la couleur
    #[arg(long, global = true)]
    pub no_color: bool,

    #[command(subcommand)]
    pub command: Command,
}

/// All subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Liste les modèles (GET /v1/models)
    Models {
        /// Une ligne par id
        #[arg(long)]
        ids: bool,
        /// JSON brut
        #[arg(long)]
        raw: bool,
    },

    /// Chat completion (POST /v1/chat/completions)
    Chat {
        message: Option<String>,
        #[arg(short = 'm', long)]
        model: Option<String>,
        #[arg(long)]
        system: Option<String>,
        #[arg(long = "max-tokens", default_value_t = 500)]
        max_tokens: u32,
        #[arg(long, default_value_t = 0.7)]
        temperature: f64,
        /// Désactive le streaming SSE
        #[arg(long = "no-stream")]
        no_stream: bool,
        /// Réponse JSON brute
        #[arg(long)]
        raw: bool,
    },

    /// Completion brute (POST /v1/completions)
    Complete {
        prompt: Option<String>,
        #[arg(short = 'm', long)]
        model: Option<String>,
        #[arg(long = "max-tokens", default_value_t = 500)]
        max_tokens: u32,
        #[arg(long, default_value_t = 0.7)]
        temperature: f64,
        #[arg(long)]
        raw: bool,
    },

    /// Embeddings (POST /v1/embeddings)
    Embed {
        text: Option<String>,
        #[arg(short = 'm', long)]
        model: Option<String>,
        #[arg(long)]
        raw: bool,
    },

    /// Compte les tokens (POST /tokenize)
    Tokenize {
        text: Option<String>,
        #[arg(short = 'm', long)]
        model: Option<String>,
        #[arg(long)]
        raw: bool,
    },

    /// État serveur (GET /health)
    Health {
        #[arg(long)]
        raw: bool,
        /// Détail latence (connect/TTFB)
        #[arg(long)]
        time: bool,
    },

    /// Version + modèle + server info (best-effort vLLM/SGLang)
    Info {
        #[arg(long)]
        raw: bool,
    },

    /// Résumé Prometheus /metrics (vllm:* / sglang:*)
    Metrics {
        #[arg(long)]
        raw: bool,
        /// grep sur le brut avant résumé
        #[arg(long)]
        filter: Option<String>,
    },

    /// Test rapide : health + 1 chat minimal (exit 0/1)
    Smoke {
        #[arg(long, default_value = "réponds OK")]
        prompt: String,
        #[arg(long)]
        model: Option<String>,
    },

    /// Mini-bench : N chats, p50/p95, tok/s
    Bench {
        #[arg(long, default_value_t = 10)]
        n: u32,
        /// Requêtes concurrentes
        #[arg(long, default_value_t = 1)]
        concurrency: u32,
        #[arg(long, default_value = "écris une phrase courte")]
        prompt: String,
        #[arg(long = "max-tokens", default_value_t = 50)]
        max_tokens: u32,
        #[arg(short = 'm', long)]
        model: Option<String>,
    },

    /// Liste les cibles configurées
    Targets,

    /// Définit la cible par défaut dans la config
    Use { name: String },

    /// Affiche la configuration résolue (clé masquée)
    Config,

    /// Génère un script de complétion shell
    Completions { shell: Shell },
}
