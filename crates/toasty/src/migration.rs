//! Schema migration types: history files, snapshots, and supporting
//! configuration.
//!
//! The migration system tracks a sequence of generated SQL migrations on disk
//! alongside the schema snapshots they were derived from. [`History`] is the
//! in-memory representation of the TOML history file; each [`HistoryEntry`]
//! records one generated migration.
//!
//! [`MigrationLadder`] tests a project's migrations: it applies them one at a
//! time, optionally on a database with rows, and compares the schema each one
//! leaves ([`DatabaseSchema`]) with the snapshot it was generated with
//! ([`SchemaDrift`]).

mod drift;
mod embed;
mod generate;
mod introspect;
mod ladder;
mod snapshot;

pub use drift::{IndexShape, SchemaDifference, SchemaDrift};
pub use embed::{MigrationFile, MigrationReport, MigrationSet};
pub use generate::{Generated, generate};
pub use introspect::{DatabaseColumn, DatabaseIndex, DatabaseSchema, DatabaseTable};
pub use ladder::{MigrationLadder, NoHook, Step};
pub use snapshot::Snapshot;
pub use toasty_core::migration::{History, HistoryEntry};
