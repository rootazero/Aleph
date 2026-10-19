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
        /// Path to old SQLite database
        #[arg(long, default_value = "~/Library/Application Support/aleph/aleph.db")]
        db: PathBuf,

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

        /// Output sessions directory
        #[arg(long, short = 'o', default_value = "~/.aleph/raw/sessions")]
        output: PathBuf,

        /// Attachments directory
        #[arg(long, default_value = "~/.aleph/raw/attachments")]
        attachments_dir: PathBuf,

        /// Number of parallel workers
        #[arg(long, default_value = "1")]
        parallel: usize,

        /// Overwrite existing sessions
        #[arg(long)]
        overwrite: bool,
    },

    /// Build metadata.db from Markdown sessions
    Index {
        /// Sessions directory to index
        #[arg(long, default_value = "~/.aleph/raw/sessions")]
        sessions_dir: PathBuf,

        /// Output database path
        #[arg(long, short = 'o', default_value = "~/.aleph/metadata.db")]
        output: PathBuf,

        /// Rebuild FTS5 index even if exists
        #[arg(long)]
        rebuild_fts: bool,
    },

    /// Verify migration integrity
    Verify {
        /// Sessions directory to verify
        #[arg(long, default_value = "~/.aleph/raw/sessions")]
        sessions_dir: PathBuf,

        /// Metadata database path
        #[arg(long, default_value = "~/.aleph/metadata.db")]
        metadata_db: PathBuf,

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
                expand_path(db)?,
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
                expand_path(output)?,
                expand_path(attachments_dir)?,
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
                expand_path(sessions_dir)?,
                expand_path(output)?,
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
                expand_path(sessions_dir)?,
                expand_path(metadata_db)?,
                original_db.map(expand_path).transpose()?,
                fix,
                remove_orphans,
            )
            .context("Verification failed")?;
        }
    }

    Ok(())
}

/// Expand ~ and environment variables in paths
fn expand_path(path: PathBuf) -> Result<PathBuf> {
    let path_str = path.to_string_lossy();

    // Expand ~
    let expanded = if path_str.starts_with("~/") {
        dirs::home_dir()
            .context("Could not determine home directory")?
            .join(&path_str[2..])
    } else if path_str == "~" {
        dirs::home_dir().context("Could not determine home directory")?
    } else {
        path
    };

    // Canonicalize if exists, otherwise return as-is
    if expanded.exists() {
        expanded
            .canonicalize()
            .with_context(|| format!("Failed to canonicalize path: {}", expanded.display()))
    } else {
        Ok(expanded)
    }
}
