# Security

Agent Dossier processes private local Codex history. Treat its SQLite index and
generated dossiers as sensitive even though common credential shapes and home
paths are redacted before persistence.

## Reporting a vulnerability

Use GitHub private vulnerability reporting for this repository. Do not include
real transcripts, indexes, credentials, or unredacted local paths. A minimal
synthetic reproducer is preferred.

If private reporting is unavailable, open an issue containing no sensitive
details and ask for a private contact channel.

If a real credential appears in Git history or an issue, revoke or rotate it
first. Deleting the current file does not remove it from existing Git history,
forks, or clones.
