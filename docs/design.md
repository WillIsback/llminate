# llminate — design

**Date:** 2026-10-19
**Status:** implemented (v0.1.0)

## Goal

Turn the private `dgx` shell script (a CLI over one DGX Spark's OpenAI-compatible
API) into a public Rust tool named **llminate** that:

1. works against **any** OpenAI-compatible server (vLLM, SGLang, ...);
2. supports **many named targets** (several GB10 nodes / base URLs) selected by name;
3. ships as a single static binary, installable via `cargo install` or prebuilt
   release tarballs.

## Naming

- Command / crate: `llminate` (verb: "to LLMinate" = to probe/test an LLM).
- `cargo install llminate` is available (the name was free on crates.io).
- Rejected: `llmctl` (already taken on crates.io and by several GitHub projects),
  `gb10` (hardware-specific).

## Configuration

File: `$LLMINATE_CONFIG`, else `$XDG_CONFIG_HOME/llminate/config.toml`,
else `~/.config/llminate/config.toml`.

```toml
default_target = "spark"

[targets.spark]
url = "http://192.168.1.87:30000"
model = "qwen3.8-flash-next"     # optional, else first /v1/models id
api_key_env = "SPARK_API_KEY"    # name of the env var holding the key
timeout_secs = 30                # optional, default 30

[targets.lab]
url = "http://100.70.22.24:30000"
api_key_env = "LAB_API_KEY"
```

Resolution precedence (highest first):

1. CLI flags: `-t/--target`, `--url`, `--api-key`, `--model`, `--timeout`
2. environment: `LLMINATE_TARGET`, `LLMINATE_BASE_URL`, `LLMINATE_API_KEY`,
   `LLMINATE_MODEL`, `LLMINATE_TIMEOUT`
3. the selected `[targets.<name>]` block
4. built-in defaults (timeout 30, model auto)

Selection rules: explicit `--target`/env → that block (error if unknown); no name →
the single configured target if exactly one, else the `default_target`; a bare
`--url`/`LLMINATE_BASE_URL` with no name yields an anonymous target.

Secrets: config references an env var name (`api_key_env`); a literal `api_key` is
allowed as an escape hatch. Nothing secret is committed.

## Commands

Same surface as `dgx`, plus target management and a parallel bench:

- `models [--ids|--raw]`
- `chat [msg] [-m M] [--system S] [--max-tokens N] [--temperature T] [--no-stream] [--raw]`
- `complete [prompt] [-m M] [--max-tokens N] [--temperature T] [--raw]`
- `embed [text] [-m M] [--raw]`
- `tokenize [text] [-m M] [--raw]`
- `health [--raw] [--time]`
- `info [--raw]`
- `metrics [--raw] [--filter SUBSTR]`
- `smoke [--prompt TXT] [--model M]`
- `bench [--n N] [--concurrency C] [--prompt TXT] [--max-tokens N] [-m M]`
- `targets`, `use <name>`, `config`, `completions <shell>`

`chat` streams SSE by default (decoding `delta.content` / `delta.reasoning`).
`--no-stream` extracts `message.content` / `message.reasoning` (reasoning-model
fallback). Every command accepts `-t/--target`. Text/prompt args fall back to stdin
when not a TTY.

## Architecture

Cargo workspace:

```
crates/llminate-core   lib: config, client, sse, stats, error
crates/llminate        bin: cli (clap), output, commands
```

- `llminate-core` has no terminal/CLI dependency; it is reusable.
- `config::resolve_with(cfg, overrides, env)` takes an injected env lookup, so
  precedence is unit-tested with no process env.
- `client::Client` wraps `reqwest` (rustls) with a bearer token and timeout.
- `sse::content_from_line` parses one `data:` line; tested in isolation.
- `stats::BenchStats` computes nearest-rank p50/p95 and tok/s.

## Errors and exit codes

Single `error::Error` enum; `exit_code()` maps:

- `2` — configuration / usage / missing API key
- `1` — network (`reqwest`), non-2xx HTTP, JSON, I/O

## Testing

- Unit tests: config precedence and `set_default_target` text edit; percentile
  stats; SSE line parsing.
- Integration tests: an in-process `axum` stub implements `/v1/models`, `/health`,
  `/version`, `/get_model_info`, `/metrics`, `/v1/chat/completions` (stream and
  non-stream, plus a reasoning-model variant), `/v1/completions`, `/v1/embeddings`,
  `/tokenize`. The real binary is run against it via `CARGO_BIN_EXE_llminate`.

## Distribution

- `cargo install llminate`.
- GitHub Releases with prebuilt tarballs for `x86_64-unknown-linux-gnu`,
  `aarch64-unknown-linux-gnu` (GB10 / DGX Spark), `aarch64-apple-darwin`.
- Tag `vX.Y.Z` triggers build + `cargo publish` (token from `CARGO_REGISTRY_TOKEN`).

## Out of scope (v0.1.0)

- Admin endpoints (vLLM/SGLang sleep/wake, KV-cache clear).
- Live watch/TUI.
- Windows builds.
