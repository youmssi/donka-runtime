# Donka Runtime

The decision-serving service of [Donka](https://github.com/youmssi/donka). It loads published
release artifacts from object storage, hot-reloads them, and evaluates decisions over REST.

This repository is a fork of [gorules/agent-public](https://github.com/gorules/agent-public)
(MIT). The upstream history is preserved; the `upstream` remote points at it and is merged
regularly. The original license and copyright notice in `LICENSE` stay as they are.

## Planned Donka changes (see donka/docs/ROADMAP.md, epic D)

| Item | Change |
|---|---|
| D1 | Rebrand; upgrade zen-engine to 2.0.1 (same pin as Studio, enforced by Studio's drift check) |
| D2 | Hashed, per-environment access tokens in `.config/project.json` (done, DNK-13: `src/data/access.rs`) |
| D3 | Connector handler (custom-node adapter) with secrets from env/vault, timeouts, retries |
| D4 | Decision-log emitter: ships input, output, release and trace to Studio asynchronously |
| D5 | Rate limiting on evaluate routes |

Changes are kept behind small extension points so upstream merges stay easy.

## Names we keep on purpose

- The Rust library is still called `agent` (only the package and binary are `donka-runtime`),
  so every source file and test stays identical to upstream.
- `x-gorules` in the generated OpenAPI documents and `application/vnd.gorules.decision` in
  decision files are contracts read by API clients and written by the editor. Renaming them
  would break compatibility; a Donka alias can be added later as an additive change.
