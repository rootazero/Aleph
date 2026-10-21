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
        ///
        /// Defaults to the pre-`ALEPH_HOME` database location.
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

        /// Output sessions directory
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,

        /// Attachments directory
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
        /// Sessions directory to index
        #[arg(long)]
        sessions_dir: Option<PathBuf>,

        /// Output database path
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,

        /// Rebuild FTS5 index even if exists
        #[arg(long)]
        rebuild_fts: bool,
    },

    /// Verify migration integrity
    Verify {
        /// Sessions directory to verify
        #[arg(long)]
        sessions_dir: Option<PathBuf>,

        /// Metadata database path
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
                expand_path(db.unwrap_or_else(default_legacy_memory_db))?,
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
                expand_path(output.unwrap_or_else(default_sessions_dir))?,
                expand_path(attachments_dir.unwrap_or_else(default_attachments_dir))?,
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
                expand_path(sessions_dir.unwrap_or_else(default_sessions_dir))?,
                expand_path(output.unwrap_or_else(default_metadata_db))?,
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
                expand_path(sessions_dir.unwrap_or_else(default_sessions_dir))?,
                expand_path(metadata_db.unwrap_or_else(default_metadata_db))?,
                original_db.map(expand_path).transpose()?,
                fix,
                remove_orphans,
            )
            .context("Verification failed")?;
        }
    }

    Ok(())
}

/// The Aleph home this tool reads and writes, resolved through the one
/// authority (`utils::paths::get_config_dir`) so a relocated `ALEPH_HOME` is
/// honoured. A migration that wrote into the real home while the server ran
/// elsewhere would look like a successful migration that changed nothing.
///
/// The fallback is the legacy home, and the last resort is the literal `~`,
/// which `expand_path` resolves — so no path here is spelled by hand.
fn default_aleph_home() -> PathBuf {
    alephcore::utils::paths::get_config_dir()
        .ok()
        .or_else(|| alephcore::utils::paths::legacy_home_aleph_path(""))
        .unwrap_or_else(|| PathBuf::from("~"))
}

/// The old SQLite database this tool exports from: the pre-`ALEPH_HOME`
/// location, because that is where the database being migrated lived.
fn default_legacy_memory_db() -> PathBuf {
    alephcore::utils::paths::legacy_home_aleph_path("data/memory.db")
        .unwrap_or_else(|| default_aleph_home().join("data").join("memory.db"))
}

/// The Markdown session directory of the current layout.
fn default_sessions_dir() -> PathBuf {
    default_aleph_home().join("raw").join("sessions")
}

/// The attachment directory of the current layout.
fn default_attachments_dir() -> PathBuf {
    default_aleph_home().join("raw").join("attachments")
}

/// The metadata index of the current layout.
fn default_metadata_db() -> PathBuf {
    default_aleph_home().join("metadata.db")
}

/// Expand ~ and environment variables in paths
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

    // Canonicalize if exists, otherwise return as-is
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

    /// The write targets follow the configured Aleph home.
    ///
    /// `get_config_dir` is a pure lookup, so this pins the layout the tool
    /// writes into without touching the filesystem.
    #[test]
    fn omitted_write_targets_follow_the_configured_aleph_home() {
        let Ok(root) = alephcore::utils::paths::get_config_dir() else {
            // No home to configure: the fallback is the legacy home.
            let legacy = alephcore::utils::paths::legacy_home_aleph_path("").unwrap();
            assert_eq!(default_aleph_home(), legacy);
            return;
        };
        assert_eq!(default_sessions_dir(), root.join("raw").join("sessions"));
        assert_eq!(
            default_attachments_dir(),
            root.join("raw").join("attachments")
        );
        assert_eq!(default_metadata_db(), root.join("metadata.db"));

        let cli = Cli::try_parse_from(["aleph-migrate", "export"]).unwrap();
        assert!(matches!(cli.command, Commands::Export { db: None, .. }));
    }

    /// The export source is the pre-`ALEPH_HOME` database.
    ///
    /// That is where the database being migrated lived, whatever the
    /// configured home is — pointing the export at the configured home would
    /// read the very layout this tool is meant to produce.
    #[test]
    fn the_export_source_is_the_legacy_database() {
        let expected = alephcore::utils::paths::legacy_home_aleph_path("data/memory.db").unwrap();
        assert_eq!(default_legacy_memory_db(), expected);
    }

    /// An explicit path overrides the default.
    #[test]
    fn explicit_paths_override_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("custom.db");
        assert_eq!(expand_path(path.clone()).unwrap(), path);
    }
}
