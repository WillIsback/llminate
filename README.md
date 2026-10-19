# llminate

> **to LLMinate** — to probe and test an LLM.

A single-binary CLI to talk to **any OpenAI-compatible LLM server** — vLLM, SGLang,
llama.cpp, or a plain router. Point it at as many endpoints as you own (several
DGX Spark / GB10 boxes, a lab box, a friend's server) and switch between them by name.

Born as a private `dgx` shell script; rebuilt in Rust as a public, portable tool.

## Install

```
cargo install llminate
```

Prebuilt binaries for `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`
(DGX Spark / GB10) and `aarch64-apple-darwin` are attached to each GitHub release.

## Quick start

```
export LLMINATE_BASE_URL=http://192.168.1.87:30000
export LLMINATE_API_KEY=sk-...

llminate models
llminate chat "what is a KV cache?"
llminate bench --n 20 --concurrency 4
```

## Multiple targets

`~/.config/llminate/config.toml`:

```toml
default_target = "spark"

[targets.spark]
url = "http://192.168.1.87:30000"
model = "qwen3.8-flash-next"
api_key_bws = "SPARK_API_KEY"     # Bitwarden Secrets Manager key name

[targets.lab]
url = "http://100.70.22.24:30000"
api_key_env = "LAB_API_KEY"       # or read from an environment variable
```

```
llminate targets          # list every target, * marks the default
llminate use lab          # set default_target
llminate chat "hi" -t lab # one-off target
```

No secret ever lives in the repo. A target picks exactly one key source:

| Field | Source |
|---|---|
| `api_key_bws = "NAME"` | Bitwarden Secrets Manager (`bws secret list`); project/server from `BWS_PROJECT_ID`/`BWS_SERVER_URL` or `bws_project`/`bws_server` |
| `api_key_cmd = "..."` | any shell command whose stdout is the key (`pass`, `op`, a `bws` wrapper…) |
| `api_key_env = "NAME"` | an environment variable |
| `api_key = "..."` | a literal (escape hatch) |

Key precedence: `--api-key` > `LLMINATE_API_KEY` > `api_key_bws` > `api_key_cmd` >
`api_key_env` > `api_key`.

## Configuration resolution

Highest wins:

1. CLI flags (`-t/--target`, `--url`, `--api-key`, `--model`, `--timeout`)
2. environment: `LLMINATE_TARGET`, `LLMINATE_BASE_URL`, `LLMINATE_API_KEY`,
   `LLMINATE_MODEL`, `LLMINATE_TIMEOUT`
3. the selected `[targets.<name>]` block
4. built-in defaults (timeout 30s, model = first id of `/v1/models`)

## Commands

| Command | Endpoint | Purpose |
|---|---|---|
| `models` | `GET /v1/models` | list models (`--ids`, `--raw`) |
| `chat` | `POST /v1/chat/completions` | chat (`--system`, `--max-tokens`, `--temperature`, `--no-stream`, `--raw`) |
| `complete` | `POST /v1/completions` | raw completion |
| `embed` | `POST /v1/embeddings` | embeddings summary |
| `tokenize` | `POST /tokenize` | token count + `max_model_len` |
| `health` | `GET /health` | status + latency (`--time`) |
| `info` | `/version`, `/get_model_info` | engine detection (vLLM / SGLang) |
| `metrics` | `GET /metrics` | Prometheus summary (`--filter`, `--raw`) |
| `probe` | `/v1/models`, `/health`, `/version`, `/metrics` | endpoint onboarding: JSON report or interactive wizard (`--json`, `--save-as`) |
| `smoke` | health + 1 chat | fast pass/fail probe |
| `bench` | N chats | p50/p95, tok/s (`--n`, `--concurrency`) |
| `targets` / `use` / `config` | — | target management |
| `completions <shell>` | — | shell completion script |

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
  "endpoints": {"/v1/models": {"status": "ok", "http_status": 200}} }
```

- Exit `0` iff `/v1/models` responds 2xx; `1` otherwise; `2` usage/config.
- `ok` = auth OK **and** at least one model.
- `auth.status`: `ok` · `not_required` · `rejected` · `required_missing` · `unreachable` · `error`.
- `engine.kind`: `vllm` · `sglang` · `openai-compatible` · `unknown`.
- With no arguments on a TTY, an interactive wizard guides you; `--save-as NAME`
  persists the endpoint as a target (key not stored — set `api_key_env`/`api_key_bws`).

## Exit codes

- `0` success
- `1` network / HTTP error
- `2` usage error or missing API key

## License

Dual-licensed under either of [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
