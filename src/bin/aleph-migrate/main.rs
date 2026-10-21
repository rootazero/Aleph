#!/usr/bin/env rust
//! # Aleph Memory Migration CLI
//!
//! Tool for migrating Aleph session data from SQLite to Markdown-based storage.
//!
//! ## Usage
//!
//! ```bash
//! # Export from SQLite to JSON
//! aleph-migrate export --db ~/old.db --output export.json
//!
//! # Transform JSON to Markdown
//! aleph-migrate transform --input export.json --output ~/.aleph/raw/sessions
//!
//! # Build metadata.db index
//! aleph-migrate index --sessions-dir ~/.aleph/raw/sessions --output ~/.aleph/metadata.db
//!
//! # Verify migration integrity
//! aleph-migrate verify --sessions-dir ~/.aleph/raw/sessions --metadata-db ~/.aleph/metadata.db
//! ```

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod export;
mod index;
mod transform;
mod verify;

#[derive(Parser)]
#[command(name = "aleph-migrate")]
#[command(about = "Migrate Aleph session data from SQLite to Markdown storage", long_about = None)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Export sessions from SQLite to JSON
    Export {
        /// Path to old SQLite database (default: <ALEPH_HOME>/data/memory.db)
        #[arg(long)]
        db: Option<PathBuf>,

        /// Output JSON file path
        #[arg(long, short = 'o', default_value = "./export.json")]
        output: PathBuf,

        /// Export only this agent's sessions
        #[arg(long)]
        agent_id: Option<String>,

        /// Export sessions after this date (YYYY-MM-DD)
        #[arg(long)]
        start_date: Option<String>,

        /// Export sessions before this date (YYYY-MM-DD)
        #[arg(long)]
        end_date: Option<String>,

        /// Filter by compression status
        #[arg(long, value_parser = ["pending", "compressed", "archived"])]
        compression_status: Option<String>,

        /// Include attachment binary data (base64)
        #[arg(long, default_value = "true")]
        include_attachments: bool,

        /// Overwrite existing output file
        #[arg(long)]
        force: bool,
    },

    /// Transform JSON export to Markdown sessions
    Transform {
        /// Input JSON file (from export)
        #[arg(long, short = 'i')]
        input: PathBuf,

        /// Output sessions directory (default: <ALEPH_HOME>/raw/sessions)
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,

        /// Attachments directory (default: <ALEPH_HOME>/raw/attachments)
        #[arg(long)]
        attachments_dir: Option<PathBuf>,

        /// Number of parallel workers
        #[arg(long, default_value = "1")]
        parallel: usize,

        /// Overwrite existing sessions
        #[arg(long)]
        overwrite: bool,
    },

    /// Build metadata.db from Markdown sessions
    Index {
        /// Sessions directory to index (default: <ALEPH_HOME>/raw/sessions)
        #[arg(long)]
        sessions_dir: Option<PathBuf>,

        /// Output database path (default: <ALEPH_HOME>/metadata.db)
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,

        /// Rebuild FTS5 index even if exists
        #[arg(long)]
        rebuild_fts: bool,
    },

    /// Verify migration integrity
    Verify {
        /// Sessions directory to verify (default: <ALEPH_HOME>/raw/sessions)
        #[arg(long)]
        sessions_dir: Option<PathBuf>,

        /// Metadata database path (default: <ALEPH_HOME>/metadata.db)
        #[arg(long)]
        metadata_db: Option<PathBuf>,

        /// Original SQLite database (for comparison)
        #[arg(long)]
        original_db: Option<PathBuf>,

        /// Auto-fix common issues
        #[arg(long)]
        fix: bool,

        /// Remove orphaned attachment references
        #[arg(long)]
        remove_orphans: bool,
    },
}

fn main() -> Result<()> {
    // Initialize logging
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp(None)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Export {
            db,
            output,
            agent_id,
            start_date,
            end_date,
            compression_status,
            include_attachments,
            force,
        } => {
            export::run_export(
                resolve_default_path(db, "data/memory.db")?,
                expand_path(output)?,
                agent_id,
                start_date,
                end_date,
                compression_status,
                include_attachments,
                force,
            )
            .context("Export failed")?;
        }

        Commands::Transform {
            input,
            output,
            attachments_dir,
            parallel,
            overwrite,
        } => {
            transform::run_transform(
                expand_path(input)?,
                resolve_default_path(output, "raw/sessions")?,
                resolve_default_path(attachments_dir, "raw/attachments")?,
                parallel,
                overwrite,
            )
            .context("Transform failed")?;
        }

        Commands::Index {
            sessions_dir,
            output,
            rebuild_fts,
        } => {
            index::run_index(
                resolve_default_path(sessions_dir, "raw/sessions")?,
                resolve_default_path(output, "metadata.db")?,
                rebuild_fts,
            )
            .context("Index failed")?;
        }

        Commands::Verify {
            sessions_dir,
            metadata_db,
            original_db,
            fix,
            remove_orphans,
        } => {
            verify::run_verify(
                resolve_default_path(sessions_dir, "raw/sessions")?,
                resolve_default_path(metadata_db, "metadata.db")?,
                original_db.map(expand_path).transpose()?,
                fix,
                remove_orphans,
            )
            .context("Verification failed")?;
        }
    }

    Ok(())
}

/// Resolve defaults through the same Aleph home as the daemon.
fn resolve_default_path(path: Option<PathBuf>, relative: &str) -> Result<PathBuf> {
    match path {
        Some(path) => expand_path(path),
        None => Ok(alephcore::utils::paths::get_config_dir()?.join(relative)),
    }
}

/// Expand an explicit user-written ~ prefix in paths.
fn expand_path(path: PathBuf) -> Result<PathBuf> {
    let path_str = path.to_string_lossy();

    // Expand ~
    let expanded = if let Some(rest) = path_str.strip_prefix("~/") {
        dirs::home_dir()
            .context("Could not determine home directory")?
            .join(rest)
    } else if path_str == "~" {
        dirs::home_dir().context("Could not determine home directory")?
    } else {
        path
    };

    // Canonicalize if exists, otherwise return as-is.
    if expanded.exists() {
        expanded
            .canonicalize()
            .with_context(|| format!("Failed to canonicalize path: {}", expanded.display()))
    } else {
        Ok(expanded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_paths_follow_the_configured_aleph_home() {
        let root = alephcore::utils::paths::get_config_dir().unwrap();
        for relative in [
            "data/memory.db",
            "raw/sessions",
            "raw/attachments",
            "metadata.db",
        ] {
            assert_eq!(
                resolve_default_path(None, relative).unwrap(),
                root.join(relative)
            );
        }
        let cli = Cli::try_parse_from(["aleph-migrate", "export"]).unwrap();
        assert!(matches!(cli.command, Commands::Export { db: None, .. }));
    }

    #[test]
    fn explicit_paths_override_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("custom.db");
        assert_eq!(
            resolve_default_path(Some(path.clone()), "metadata.db").unwrap(),
            path
        );
    }
}
