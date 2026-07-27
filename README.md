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

## Install

Agent Dossier requires Rust 1.97 or newer.

```bash
cargo install --git https://github.com/rudrankriyam/agent-dossier --locked
```

You can also build a local checkout with `cargo build --release --locked`.

## Usage

Build or incrementally refresh the index:

```console
$ agent-dossier index
indexed 412 files (0 unchanged, 0 deleted) · 412 sessions · 28391 events · 0 warnings
index: <OS cache>/agent-dossier/index.sqlite
```

Agent Dossier reads `~/.codex/sessions` and `~/.codex/archived_sessions` by
default. Repeat `--source` to index explicit Codex JSONL roots instead.

Compile a bounded evidence packet:

```console
$ agent-dossier dossier "Why was the release paused?"
```

The dossier is cited evidence for Codex or a human to reason over. It is not a
generated answer. Use `--format json` for machine-readable output and
`--help` for bounds and path overrides.

## Privacy

The SQLite index stays local. Agent Dossier removes common credential shapes,
terminal controls, and the current home-directory prefix before persistence.
The index is derived data: delete it at any time and rebuild it from Codex
JSONL.

Redaction is defense in depth, not a guarantee. Review dossiers before sharing
them, and never commit a real index or transcript.

## Benchmark

The current blinded result is 4/10 strict passes for Codex with Agent Dossier
versus 6/10 for native Codex, with a 4.67x median speedup and 94% fewer input
tokens. See [the benchmark methodology and full aggregate
table](docs/benchmark.md).

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-features --locked
```

See [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request. Security
reports follow [SECURITY.md](SECURITY.md).

## License

MIT
