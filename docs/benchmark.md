# Benchmark

Agent Dossier is experimental. Speed has cleared the project gate; accuracy has
not.

## Result

Ten blinded questions were drawn from a private Codex archive. They covered:

- what worked last time;
- why a decision was made;
- what remained unfinished;
- cross-session reconstruction;
- model and session metadata; and
- attachment lineage.

The questions referenced 12 source sessions with no overlap with the
development set. Both arms used `gpt-5.6-sol` at medium reasoning effort. Four
independent graders received balanced, masked answers and applied the same
strict required-fact, citation, hallucination, and usefulness rubric.

| Metric | Native Codex | Codex with Agent Dossier |
|---|---:|---:|
| Strict accuracy passes | 6/10 | 4/10 |
| Total answer time | 1,408.4s | 274.7s |
| Median relative speed | baseline | 4.67x faster |
| Slowest per-case relative speed | baseline | 2.29x faster |
| Input tokens | 13,411,540 | 808,859 |
| Input-token change | baseline | 94.0% fewer |
| History-investigation tool calls | 126 | 0 |
| Deterministic retrieval time | — | 9.6s total |

The compact index covered approximately 12.4 GB of Codex JSONL:

| Index metric | Result |
|---|---:|
| Warm-filesystem clean build | 21.74s |
| Indexed events | 770,929 |
| SQLite index | 576 MB |

## Interpretation

Agent Dossier is already a useful latency and token compressor, but it is not
yet a reliable replacement for native Codex history reconstruction. Its
current failure modes include incomplete chronology, missing model metadata,
attachment lineage, and evidence being dropped by packet limits.

The release-quality gate is:

1. at least 6/10 strict accuracy on the frozen holdout;
2. greater than 2x median speedup;
3. no increase in critical hallucinations; and
4. exact provenance for every factual conclusion.

Only aggregate results and synthetic fixtures belong in this repository. The
private questions, transcripts, attachments, local paths, and ground truth do
not.
