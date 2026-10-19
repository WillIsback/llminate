# Spec — `llminate probe` (endpoint onboarding probe)

**Status:** proposed
**Target version:** 0.3.0

## Goal

One command that inspects a single OpenAI-compatible endpoint and returns a stable,
machine-readable report, so a backend (any language) can implement an "Add a model
server" flow: validate the endpoint and auto-detect engine, models and available
endpoints, without reimplementing vLLM/SGLang glue or secret handling.

Two consumers:

- **Machine** (backend/CI): `llminate probe --json` → one JSON document, stable contract.
- **Human** (ops/dev): interactive wizard when attached to a TTY, progressive output
  otherwise.

## Command surface

```
llminate probe [OPTIONS]

Target (global):  -t/--target NAME | --url URL [--api-key K]
Global:           --timeout N, --no-color
Local:            --json              one JSON document (no wizard, no colour)
                  --interactive, -i   force the wizard (requires a TTY)
                  --save-as NAME      persist the probed endpoint as a target on success
                  --set-default       with --save-as, also set default_target
```

Rules:

- Wizard activation: forced by `--interactive`; otherwise automatic only when both
  stdin and stdout are TTYs, `--json` is absent, **and** no target/`--url` was supplied
  (nothing to probe yet). Supplying `-t`/`--url` implies a one-shot non-interactive run.
- `--json` never prompts; stdout carries the document only (prompts/status, if any, go
  to stderr).
- The API key should be provided via `LLMINATE_API_KEY` or `--api-key`; the wizard reads
  it hidden (`dialoguer::Password`).

## JSON contract (v1)

```json
{
  "probe_version": 1,
  "ok": true,
  "target": "spark",
  "url": "http://192.168.1.87:30000",
  "auth": { "status": "ok", "http_status": 200 },
  "engine": { "kind": "vllm", "version": "0.32.0rc1.dev301+g1ac942555" },
  "models": [{ "id": "nvidia/…-NVFP4", "owned_by": "vllm" }],
  "latency_ms": 14,
  "health": { "status": "ok", "latency_ms": 3 },
  "endpoints": {
    "/v1/models":      { "status": "ok",      "http_status": 200 },
    "/health":         { "status": "ok",      "http_status": 200 },
    "/version":        { "status": "ok",      "http_status": 200 },
    "/get_model_info": { "status": "missing", "http_status": 404 },
    "/metrics":        { "status": "ok",      "http_status": 200 }
  },
  "errors": []
}
```

- `probe_version` pins the contract for consumers.
- `target` is `null` for an ad-hoc (`--url`) endpoint.
- Probe GET-only endpoints (POST-only `/tokenize` is not probed without side effects).

## Behaviours

- `auth.status`:
  - `ok` — `/v1/models` 2xx with a key supplied
  - `not_required` — `/v1/models` 2xx without a key
  - `rejected` — 401 with a key supplied
  - `required_missing` — 401 without a key
- `engine.kind`: `vllm` (via `/version`) · `sglang` (via `/get_model_info`) ·
  `openai-compatible` · `unknown` (existing detection logic, reused).
- Endpoint status: `ok` | `missing` (404/405) | `denied` (401/403) | `unreachable`
  (network). Optional probes never block.
- `latency_ms`: measured on the `/v1/models` call; `health.latency_ms` on `/health`.
- `ok` = auth OK **and** `models` non-empty. This is the backend's fine-grained verdict.
- Tolerant: any optional endpoint missing is recorded, not fatal.

### Exit codes

| Situation | Exit |
|---|---|
| `/v1/models` reached with 2xx (whatever `ok`) | 0 |
| auth rejected / required_missing, endpoint not OpenAI-compatible | 1 |
| network error (unreachable, timeout) | 1 |
| usage error / missing config / bad secret source | 2 |

## Failure matrix (backend → UI message)

| Signal | Meaning | Suggested message |
|---|---|---|
| exit 1, `auth.status = rejected` | wrong key | "Clé refusée — vérifie l'API key." |
| exit 1, `auth.status = required_missing` | key needed | "Cet endpoint exige une API key." |
| exit 1, `errors[].probe = /v1/models`, unreachable | bad URL/firewall | "Endpoint injoignable — vérifie l'URL." |
| exit 0, `models = []` | reachable, no model | "Aucun modèle servi." |
| exit 1, `ok = false`, `/v1/models` = missing | not OpenAI-compatible | "Répond mais pas en API OpenAI-compatible." |
| exit 0, `ok = true` | success | show engine + models + endpoints |

## CLI user experience

### Interactive wizard (TTY, no `--json`)

```
$ llminate probe
URL de l'endpoint [http://192.168.1.87:30000]: ⏎
Clé API (vide si non requise): ********
Analyse…
  ✓ /v1/models        200   14 ms
  ✓ /health           200
  ✓ /version          200   vllm 0.32.0rc1.dev301+g1ac942555
  · /get_model_info   404
  ✓ /metrics          200

  Endpoint OpenAI-compatible · auth ok · latence 14 ms
  moteur : vllm 0.32.0rc1.dev301+g1ac942555
  modèles (1) : nvidia/NVIDIA-Nemotron-3.5-Super-VL-120B-A12B-NVFP4

Enregistrer comme cible llminate ? (o/N) o
Nom de la cible : spark
✓ cible « spark » ajoutée (~/.config/llminate/config.toml)
```

On failure: targeted message + retry loop (re-prompt URL/key).

### Non-interactive

```
llminate probe -t spark                  # human summary (TTY)
llminate probe --url U --api-key K       # ad-hoc
llminate probe --url U --json            # machine contract
llminate probe --url U --save-as spark   # persist target on success
```

Progress lines are printed as each probe completes; under `--json` everything is
collected silently and the single document is printed at the end.

## Implementation

- **Core** (`crates/llminate-core/src/probe.rs`), reusable by Rust apps:
  `pub struct ProbeReport` (`Serialize`) and `pub async fn probe(client: &Client) -> ProbeReport`.
  Reuses `Client`, `get_opt`, and the existing engine-detection logic.
- **CLI** (`crates/llminate/src/commands/probe.rs`):
  - collects the report, then renders either the JSON document, the progressive/human
    view, or the wizard.
  - `reqwest` timeouts per probe (short, e.g. `--timeout`).
- **New dependency**: `dialoguer` (CLI crate only; password input, confirm, input).
- **Secret hygiene**: prefer `LLMINATE_API_KEY`; never echo the key.

## Tests

- Unit (core): `auth.status` classification (200/401 × key/–); `engine.kind`
  classification (vllm / sglang / generic / unknown) from fixture bodies; endpoint
  status mapping (200 → ok, 404 → missing, network → unreachable).
- Integration (CLI): against the axum stub, assert the full JSON contract
  (`ok`, `engine.kind = vllm`, `models[0].id`, `endpoints`), plus a 401 stub path
  (auth `rejected`/`required_missing`) and `--save-as` writing a target.

## Out of scope (v1)

- Active/functional probes (mini chat, embeddings, tokenize).
- Multi-endpoint sweeps.
- HTML reporting.
