# Agent Dossier

**A local evidence compiler for Codex history.**

Agent Dossier streams Codex JSONL on your machine, reconstructs task
provenance, removes inherited duplicates, and emits bounded Markdown or JSON
with exact source citations.

> [!WARNING]
> Agent Dossier is experimental. On a private 10-question blinded holdout,
> Codex answers supplied with dossiers passed 4/10 strict cases versus 6/10
> for native Codex history reconstruction. The dossier workflow was 4.67x
> faster at the median and used 94% fewer input tokens. Accuracy is the current
> blocker.

## Scope

This project intentionally supports Codex history only. It is not generic
chat search, an autonomous answerer, or a replacement for `codex resume`.

The public repository never contains real transcripts. Tests and examples use
synthetic Codex fixtures.

## Planned first commands

```console
$ agent-dossier index
$ agent-dossier dossier "Why was the release paused?"
```

The dossier is cited evidence for Codex or a human to reason over. It is not a
generated answer.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

## License

MIT
