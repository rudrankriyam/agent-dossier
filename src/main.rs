use std::path::PathBuf;

use agent_dossier::dossier::DossierRequest;
use agent_dossier::index::{default_codex_sources, default_index_path};
use agent_dossier::{CodexIndex, render_markdown};
use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "agent-dossier",
    version,
    about = "A local evidence compiler for Codex history"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Build or incrementally refresh the local Codex history index.
    Index {
        /// SQLite index path. Defaults to the OS cache directory.
        #[arg(long)]
        db: Option<PathBuf>,
        /// Codex JSONL root. Repeat to add active and archived roots.
        #[arg(long = "source")]
        sources: Vec<PathBuf>,
        /// Discard the derived index and rebuild it from source JSONL.
        #[arg(long)]
        rebuild: bool,
        /// Output format for indexing statistics.
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
    },
    /// Compile cited evidence for a question about prior Codex work.
    Dossier {
        /// Natural-language question used only for deterministic retrieval.
        query: String,
        /// SQLite index path. Defaults to the OS cache directory.
        #[arg(long)]
        db: Option<PathBuf>,
        /// Maximum distinct sessions in the dossier.
        #[arg(long, default_value_t = 5)]
        sessions: usize,
        /// Maximum evidence events in the dossier.
        #[arg(long, default_value_t = 12)]
        evidence: usize,
        /// Reserved neighboring-event radius (0-3).
        #[arg(long, default_value_t = 1)]
        context: usize,
        /// Dossier output format.
        #[arg(long, value_enum, default_value_t = Format::Markdown)]
        format: Format,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Markdown,
    Json,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Index {
            db,
            sources,
            rebuild,
            format,
        } => {
            let db = db.unwrap_or(default_index_path()?);
            let sources = if sources.is_empty() {
                default_codex_sources()
            } else {
                sources
            };
            let mut index = CodexIndex::open(&db)?;
            let stats = if rebuild {
                index.rebuild(&sources)?
            } else {
                index.refresh(&sources)?
            };
            match format {
                Format::Json => println!("{}", serde_json::to_string_pretty(&stats)?),
                Format::Text | Format::Markdown => println!(
                    "indexed {} files ({} unchanged, {} deleted) · {} sessions · {} events · {} warnings\nindex: {}",
                    stats.files_indexed,
                    stats.files_unchanged,
                    stats.files_deleted,
                    stats.sessions,
                    stats.events,
                    stats.warnings,
                    db.display()
                ),
            }
        }
        Command::Dossier {
            query,
            db,
            sessions,
            evidence,
            context,
            format,
        } => {
            let db = db.unwrap_or(default_index_path()?);
            let index = CodexIndex::open(&db)?;
            let mut request = DossierRequest::new(query);
            request.max_sessions = sessions;
            request.max_evidence = evidence;
            request.context = context;
            let dossier = index.dossier(request)?;
            match format {
                Format::Json => println!("{}", serde_json::to_string_pretty(&dossier)?),
                Format::Text | Format::Markdown => print!("{}", render_markdown(&dossier)),
            }
        }
    }
    Ok(())
}
