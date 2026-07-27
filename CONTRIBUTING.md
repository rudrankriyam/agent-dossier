# Contributing

Agent Dossier is intentionally optimized for Codex history. Changes should
improve Codex parsing, provenance, retrieval quality, privacy, or local
performance without introducing a generic provider abstraction.

## Before a pull request

1. Keep fixtures synthetic. Never commit Codex transcripts, indexes, local
   paths, credentials, or private benchmark questions.
2. Add a focused test for behavior changes.
3. Run:

   ```bash
   cargo fmt --all -- --check
   cargo clippy --all-targets --all-features --locked -- -D warnings
   cargo test --all-features --locked
   ```

4. Explain the user-visible outcome and any privacy or provenance tradeoff.

Small, reviewable commits are welcome. Benchmark claims must include enough
methodology to reproduce the public aggregate without publishing private
history.
