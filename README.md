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
api_key_env = "SPARK_API_KEY"

[targets.lab]
url = "http://100.70.22.24:30000"
api_key_env = "LAB_API_KEY"
```

```
llminate targets          # list every target, * marks the default
llminate use lab          # set default_target
llminate chat "hi" -t lab # one-off target
```

No secret ever lives in the repo: the config references an **environment variable**
name (`api_key_env`). A literal `api_key = "..."` is accepted as an escape hatch.

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
| `smoke` | health + 1 chat | fast pass/fail probe |
| `bench` | N chats | p50/p95, tok/s (`--n`, `--concurrency`) |
| `targets` / `use` / `config` | — | target management |
| `completions <shell>` | — | shell completion script |

## Exit codes

- `0` success
- `1` network / HTTP error
- `2` usage error or missing API key

## License

Dual-licensed under either of [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
