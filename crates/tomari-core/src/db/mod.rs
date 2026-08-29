//! SQLite persistence layer.
//!
//! A single [`Database`] owns the connection behind a mutex so it can be stored
//! in shared application state and used from multiple threads. Repository
//! methods are implemented across the submodules of this module.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, Transaction, TransactionBehavior};

use crate::domain::AppSettings;
use crate::domain::keyboard::{Hotkey, ModifierRule};
use crate::error::{Error, Result};

mod keyboard;
mod meta;
mod placements;
mod settings;

/// A thread-safe handle to the on-disk SQLite database.
pub struct Database {
    conn: Mutex<Connection>,
}

/// The outcome of decoding the canonical settings row during a persisted-state
/// preflight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistedSettings {
    /// No canonical settings row is stored.
    Missing,
    /// The canonical row is present and fully decoded.
    Ready(AppSettings),
    /// The row is present and scalar-readable, but its JSON is not compatible
    /// with this build.
    UnreadableJson { message: String },
}

/// Stored and skipped row totals for one persisted table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PersistedRowCounts {
    pub stored: usize,
    pub skipped: usize,
}

impl PersistedRowCounts {
    fn accepted(self) -> usize {
        self.stored.saturating_sub(self.skipped)
    }
}

/// A single-snapshot read of every source that can affect startup automation
/// or later window actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedStatePreflight {
    pub settings: PersistedSettings,
    pub hotkeys: Vec<Hotkey>,
    pub modifier_rules: Vec<ModifierRule>,
    pub settings_rows: PersistedRowCounts,
    pub hotkey_rows: PersistedRowCounts,
    pub modifier_rule_rows: PersistedRowCounts,
    pub meta_rows: PersistedRowCounts,
    pub window_placement_rows: PersistedRowCounts,
}

/// How an explicit recovery reset treats readable keyboard automation rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupConfigurationReset {
    /// Preserve hotkeys and modifier rules only when every stored row decodes.
    PreserveReadableAutomation,
    /// Replace all hotkeys and modifier rules with the supplied safe defaults.
    ReplaceAutomation,
}

impl PersistedStatePreflight {
    /// Whether all application tables were empty in the inspected snapshot.
    ///
    /// Stored rows count even when their JSON or placement payload was skipped,
    /// so damaged data can never be mistaken for a first run.
    pub fn is_pristine(&self) -> bool {
        [
            self.settings_rows,
            self.hotkey_rows,
            self.modifier_rule_rows,
            self.meta_rows,
            self.window_placement_rows,
        ]
        .into_iter()
        .all(|rows| rows.stored == 0)
    }
}

pub(super) struct DecodedRows<T> {
    pub values: Vec<T>,
    pub counts: PersistedRowCounts,
}

impl<T> DecodedRows<T> {
    fn debug_assert_consistent(&self) {
        debug_assert_eq!(self.values.len(), self.counts.accepted());
    }
}

fn preflight_persisted_state(conn: &Connection) -> Result<PersistedStatePreflight> {
    let (settings, settings_rows) = settings::preflight_settings(conn)?;
    let hotkeys = keyboard::read_hotkeys(conn)?;
    let modifier_rules = keyboard::read_modifier_rules(conn)?;
    let meta_rows = meta::preflight_meta(conn)?;
    let window_placements = placements::preflight_window_placements(conn)?;

    hotkeys.debug_assert_consistent();
    modifier_rules.debug_assert_consistent();
    window_placements.debug_assert_consistent();

    Ok(PersistedStatePreflight {
        settings,
        hotkeys: hotkeys.values,
        modifier_rules: modifier_rules.values,
        settings_rows,
        hotkey_rows: hotkeys.counts,
        modifier_rule_rows: modifier_rules.counts,
        meta_rows,
        window_placement_rows: window_placements.counts,
    })
}

fn is_row_scalar_error(error: &Error) -> bool {
    matches!(
        error,
        Error::Database(
            rusqlite::Error::InvalidColumnType(..)
                | rusqlite::Error::IntegralValueOutOfRange(..)
                | rusqlite::Error::FromSqlConversionFailure(..)
        )
    )
}

impl Database {
    /// Open (creating if needed) the database at `path` and run migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    /// Open an in-memory database — handy for tests.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        // Wait for a lock held by another connection (e.g. a concurrent writer
        // under WAL) instead of failing immediately with `SQLITE_BUSY`.
        conn.busy_timeout(Duration::from_secs(5))?;
        // Check the complete existing database before WAL setup or migrations
        // can write to it. SQLite may report damage as a successful pragma row,
        // so `quick_check` validates the result as well as propagating SQL errors.
        quick_check(&conn)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        Ok(db)
    }

    /// Run lock-guarded work against the connection.
    ///
    /// Recovers the guard from a poisoned mutex rather than panicking: a panic
    /// while a query was running poisons the lock, and under the release
    /// profile's `panic = "abort"` propagating that would silently terminate a
    /// resident app. The connection itself stays usable (a panicking statement
    /// does not corrupt it), so taking the guard back lets later queries proceed.
    fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let guard = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        f(&guard)
    }

    /// Total row count of `table`. `table` is only ever an in-crate string
    /// literal (never user input), so interpolating it carries no injection
    /// risk. Used to compare against a decoded list and spot silently-skipped
    /// (undecodable) rows.
    fn count_rows(&self, table: &str) -> Result<usize> {
        self.with_conn(|conn| {
            let n: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })?;
            Ok(n as usize)
        })
    }

    /// Read every persisted application table under one SQLite snapshot.
    ///
    /// JSON payloads that belong to independently editable rows are counted and
    /// skipped when they no longer decode. SQLite, schema, and scalar conversion
    /// errors remain hard failures: treating those as isolated rows could enable
    /// automation from a partial or internally inconsistent snapshot.
    pub fn preflight_persisted_state(&self) -> Result<PersistedStatePreflight> {
        self.with_conn(|conn| {
            let tx = Transaction::new_unchecked(conn, TransactionBehavior::Deferred)?;
            let report = preflight_persisted_state(&tx)?;
            tx.commit()?;
            Ok(report)
        })
    }

    /// Whether any persisted application data exists in the migrated schema.
    ///
    /// An empty database still has tables and a non-zero SQLite `user_version`,
    /// neither of which is user data. Check every table Tomari persists into in
    /// one query so first-run detection cannot overlook internal metadata or a
    /// remembered window placement.
    pub fn has_persisted_data(&self) -> Result<bool> {
        self.with_conn(|conn| {
            let exists = conn.query_row(
                "SELECT EXISTS (
                   SELECT 1 FROM settings
                   UNION ALL SELECT 1 FROM hotkeys
                   UNION ALL SELECT 1 FROM modifier_rules
                   UNION ALL SELECT 1 FROM meta
                   UNION ALL SELECT 1 FROM window_placements
                 )",
                [],
                |row| row.get(0),
            )?;
            Ok(exists)
        })
    }

    /// Seed the first-run defaults — hotkeys, modifier rules and the settings
    /// row — in a single transaction, so a failure part-way through rolls back
    /// and never leaves a half-populated database.
    ///
    /// Only call this once a real first run has been confirmed: the settings row
    /// is absent and [`has_persisted_data`](Self::has_persisted_data) is false. A
    /// *read failure* while checking must not be treated as a first run: data may
    /// exist but be momentarily unreadable, and seeding then would overwrite a
    /// real user's configuration. Writing all rows atomically also guarantees
    /// the settings row (the first-run marker) only appears if the accompanying
    /// hotkeys and rules landed too.
    pub fn seed_defaults(
        &self,
        hotkeys: &[Hotkey],
        rules: &[ModifierRule],
        settings: &AppSettings,
    ) -> Result<()> {
        self.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            for hk in hotkeys {
                keyboard::write_hotkey(&tx, hk)?;
            }
            for rule in rules {
                keyboard::write_modifier_rule(&tx, rule)?;
            }
            settings::write_settings(&tx, settings)?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Repair the persisted startup configuration in one write transaction.
    ///
    /// Metadata and window placements are hard-read before the first write, so
    /// a structural failure outside the startup automation tables cannot be
    /// hidden by a successful settings reset. In preserve mode, any skipped or
    /// scalar-unreadable keyboard row escalates to a full automation
    /// replacement. Missing automation tables still fail the replacement SQL,
    /// rolling every earlier delete/write back with the transaction.
    pub fn reset_startup_configuration(
        &self,
        mode: StartupConfigurationReset,
        hotkeys: &[Hotkey],
        rules: &[ModifierRule],
        settings: &AppSettings,
    ) -> Result<PersistedStatePreflight> {
        self.with_conn(|conn| {
            let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;

            // These tables are deliberately not part of a settings reset. Read
            // every scalar first so Reset cannot mutate automation and only
            // afterward discover that the same launch would remain unsafe.
            let _ = meta::preflight_meta(&tx)?;
            let _ = placements::preflight_window_placements(&tx)?;

            let preserve_automation = match mode {
                StartupConfigurationReset::ReplaceAutomation => false,
                StartupConfigurationReset::PreserveReadableAutomation => {
                    match (
                        keyboard::read_hotkeys(&tx),
                        keyboard::read_modifier_rules(&tx),
                    ) {
                        (Ok(hotkeys), Ok(rules)) => {
                            hotkeys.counts.skipped == 0 && rules.counts.skipped == 0
                        }
                        (Err(error), _) | (_, Err(error)) if is_row_scalar_error(&error) => {
                            tracing::warn!(
                                %error,
                                "automation rows are unreadable; replacing them during settings recovery"
                            );
                            false
                        }
                        (Err(error), _) | (_, Err(error)) => return Err(error),
                    }
                }
            };

            if preserve_automation {
                settings::write_settings(&tx, settings)?;
            } else {
                tx.execute("DELETE FROM hotkeys", [])?;
                tx.execute("DELETE FROM modifier_rules", [])?;
                tx.execute("DELETE FROM settings", [])?;
                for hotkey in hotkeys {
                    keyboard::write_hotkey(&tx, hotkey)?;
                }
                for rule in rules {
                    keyboard::write_modifier_rule(&tx, rule)?;
                }
                settings::write_settings(&tx, settings)?;
            }

            let report = preflight_persisted_state(&tx)?;
            let settings_complete = matches!(
                &report.settings,
                PersistedSettings::Ready(stored) if stored == settings
            ) && report.settings_rows
                == (PersistedRowCounts {
                    stored: 1,
                    skipped: 0,
                });
            if !settings_complete
                || report.hotkey_rows.skipped != 0
                || report.modifier_rule_rows.skipped != 0
            {
                return Err(Error::invalid(
                    "configuration reset",
                    "the repaired startup snapshot is still incomplete",
                ));
            }

            tx.commit()?;
            Ok(report)
        })
    }

    fn migrate(&self) -> Result<()> {
        self.with_conn(|conn| apply_migrations(conn, MIGRATIONS))
    }
}

/// Verify that SQLite considers every page structurally sound.
///
/// `quick_check(1)` bounds diagnostics to the first problem. A clean database
/// returns exactly one row containing `ok`; no row, an extra row, or any other
/// text is itself an integrity failure even though the pragma query succeeded.
fn quick_check(conn: &Connection) -> Result<()> {
    let mut statement = conn.prepare("PRAGMA quick_check(1)")?;
    let mut rows = statement.query([])?;
    let mut results = Vec::new();
    while let Some(row) = rows.next()? {
        results.push(row.get::<_, String>(0)?);
    }
    match results.as_slice() {
        [result] if result == "ok" => Ok(()),
        [] => Err(Error::DatabaseIntegrity(
            "quick_check returned no result".into(),
        )),
        reports => Err(Error::DatabaseIntegrity(reports.join("; "))),
    }
}

/// Bring a database up to date by applying, in order, every migration its
/// stored `user_version` has not seen yet.
///
/// Each step runs in its own transaction that also stamps the version it
/// reached, so a failure rolls that step back entirely (`PRAGMA user_version`
/// is transactional and reverts too) and a crash between steps leaves a
/// consistent intermediate version that the next launch resumes from. A
/// database stamped *ahead* of `migrations` was written by a newer app and is
/// refused outright: pretending it matches the current schema risks silent
/// data loss. A *negative* version (SQLite stores whatever was stamped) is
/// equally meaningless and refused — indexing the list with it would panic,
/// and the release profile's `panic = "abort"` would turn that into a silent
/// exit instead of the intended launch error.
///
/// Takes the migration list as a parameter so tests can drive it with
/// synthetic histories; production always passes [`MIGRATIONS`].
fn apply_migrations(conn: &Connection, migrations: &[&str]) -> Result<()> {
    let latest = migrations.len() as i32;
    let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version < 0 || version > latest {
        return Err(version_out_of_range(version, latest));
    }
    for next in (version + 1)..=latest {
        apply_step(conn, migrations, next)?;
    }
    Ok(())
}

/// The launch error for a stored schema version this binary cannot handle.
fn version_out_of_range(version: i32, latest: i32) -> Error {
    Error::Migration(format!(
        "database schema version {version} is outside what this app supports \
         (expected 0 to {latest}); if it is newer, please update the app"
    ))
}

/// Apply the single migration that brings the schema to version `next`, under
/// an *immediate* (write-locking) transaction.
///
/// The version is re-read once the lock is held: two app instances can race
/// through [`apply_migrations`]' initial read at launch — the database opens
/// before the single-instance guard engages — and the loser must skip steps
/// the winner already committed rather than re-run them (a re-run would at
/// best fail on existing DDL and at worst re-apply a non-idempotent data
/// transformation). Skipping drops the transaction unstarted-equivalent: it
/// rolls back having written nothing.
///
/// The re-read repeats the out-of-range check too: the racing instance could
/// be a *newer* binary that advanced the version past what this one supports,
/// and skipping silently would report a successful launch against a schema
/// this binary does not understand.
fn apply_step(conn: &Connection, migrations: &[&str], next: i32) -> Result<()> {
    let latest = migrations.len() as i32;
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current < 0 || current > latest {
        return Err(version_out_of_range(current, latest));
    }
    if current >= next {
        return Ok(());
    }
    tx.execute_batch(migrations[(next - 1) as usize])
        .map_err(|source| Error::MigrationStep { step: next, source })?;
    tx.pragma_update(None, "user_version", next)?;
    tx.commit()?;
    Ok(())
}

/// Collect mapped rows, skipping any row whose stored JSON no longer
/// deserializes (corruption, or a value written by a newer app version).
/// One bad row must not take the whole list — and with it every hotkey or
/// rule — down. Real query errors still propagate.
fn collect_valid_rows<T>(
    rows: impl Iterator<Item = rusqlite::Result<T>>,
    entity: &str,
) -> Result<Vec<T>> {
    let mut out = Vec::new();
    for row in rows {
        match row {
            Ok(value) => out.push(value),
            Err(rusqlite::Error::FromSqlConversionFailure(_, _, e)) => {
                tracing::warn!(entity, error = %e, "skipping a stored row that does not deserialize");
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(out)
}

/// Ordered schema migrations: `MIGRATIONS[n]` upgrades a database at
/// `user_version == n` to `n + 1`. To change the schema, append a new SQL
/// batch (and never edit an existing entry — released builds have already
/// applied those); the schema version a binary writes is simply the list's
/// length, so it needs no separate bump.
const MIGRATIONS: &[&str] = &[MIGRATION_V1, MIGRATION_V2, MIGRATION_V3];

/// `0 → 1`: the initial schema. `hyper` defaults to `0` so callers can omit it
/// and get the "not a hyper key" behaviour. `IF NOT EXISTS` tolerates a
/// database created by a pre-versioning build that has the tables but still
/// reads `user_version == 0`; later migrations should use plain DDL.
const MIGRATION_V1: &str = r#"
CREATE TABLE IF NOT EXISTS hotkeys (
    id          TEXT    PRIMARY KEY,
    label       TEXT    NOT NULL,
    accelerator TEXT    NOT NULL,
    action      TEXT    NOT NULL,
    enabled     INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS modifier_rules (
    id        TEXT    PRIMARY KEY,
    label     TEXT    NOT NULL,
    modifier  TEXT    NOT NULL,
    side      TEXT    NOT NULL,
    remap_to  TEXT,
    tap       TEXT    NOT NULL,
    enabled   INTEGER NOT NULL,
    hyper     INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS settings (
    id   INTEGER PRIMARY KEY CHECK (id = 1),
    data TEXT    NOT NULL
);
"#;

/// `1 → 2`: the `meta` key/value table, for small app-internal records (the
/// permission snapshot) that must stay out of the `settings` row the frontend
/// round-trips.
const MIGRATION_V2: &str = r#"
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

/// `2 → 3`: two named, display-relative homes per application. The frame is
/// stored as JSON because it is already a domain value crossing the frontend
/// boundary, while the application and slot remain queryable columns.
const MIGRATION_V3: &str = r#"
CREATE TABLE window_placements (
    bundle_id TEXT NOT NULL,
    app_name  TEXT NOT NULL,
    slot      TEXT NOT NULL CHECK (slot IN ('primary', 'secondary')),
    frame     TEXT NOT NULL,
    PRIMARY KEY (bundle_id, slot)
);
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// The version a fully migrated database reads.
    const SCHEMA_VERSION: i32 = MIGRATIONS.len() as i32;

    /// Frozen snapshots of every schema version a build has shipped with:
    /// `VERSION_FIXTURES[n - 1]` recreates a version-`n` database verbatim.
    /// Deliberately independent of `MIGRATIONS` — deriving fixtures from the
    /// live list would let an accidental edit to a shipped migration rewrite
    /// the "old database" being tested and hide the incompatibility. Append a
    /// snapshot when a new version ships; never touch existing entries.
    const VERSION_FIXTURES: &[&str] = &[
        // v1: the initial schema.
        r#"
CREATE TABLE hotkeys (
    id          TEXT    PRIMARY KEY,
    label       TEXT    NOT NULL,
    accelerator TEXT    NOT NULL,
    action      TEXT    NOT NULL,
    enabled     INTEGER NOT NULL
);

CREATE TABLE modifier_rules (
    id        TEXT    PRIMARY KEY,
    label     TEXT    NOT NULL,
    modifier  TEXT    NOT NULL,
    side      TEXT    NOT NULL,
    remap_to  TEXT,
    tap       TEXT    NOT NULL,
    enabled   INTEGER NOT NULL,
    hyper     INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE settings (
    id   INTEGER PRIMARY KEY CHECK (id = 1),
    data TEXT    NOT NULL
);

PRAGMA user_version = 1;
"#,
        // v2: v1 plus the `meta` key/value table.
        r#"
CREATE TABLE hotkeys (
    id          TEXT    PRIMARY KEY,
    label       TEXT    NOT NULL,
    accelerator TEXT    NOT NULL,
    action      TEXT    NOT NULL,
    enabled     INTEGER NOT NULL
);

CREATE TABLE modifier_rules (
    id        TEXT    PRIMARY KEY,
    label     TEXT    NOT NULL,
    modifier  TEXT    NOT NULL,
    side      TEXT    NOT NULL,
    remap_to  TEXT,
    tap       TEXT    NOT NULL,
    enabled   INTEGER NOT NULL,
    hyper     INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE settings (
    id   INTEGER PRIMARY KEY CHECK (id = 1),
    data TEXT    NOT NULL
);

CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

PRAGMA user_version = 2;
"#,
        // v3: v2 plus remembered per-application window positions.
        r#"
CREATE TABLE hotkeys (
    id          TEXT    PRIMARY KEY,
    label       TEXT    NOT NULL,
    accelerator TEXT    NOT NULL,
    action      TEXT    NOT NULL,
    enabled     INTEGER NOT NULL
);

CREATE TABLE modifier_rules (
    id        TEXT    PRIMARY KEY,
    label     TEXT    NOT NULL,
    modifier  TEXT    NOT NULL,
    side      TEXT    NOT NULL,
    remap_to  TEXT,
    tap       TEXT    NOT NULL,
    enabled   INTEGER NOT NULL,
    hyper     INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE settings (
    id   INTEGER PRIMARY KEY CHECK (id = 1),
    data TEXT    NOT NULL
);

CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE window_placements (
    bundle_id TEXT NOT NULL,
    app_name  TEXT NOT NULL,
    slot      TEXT NOT NULL CHECK (slot IN ('primary', 'secondary')),
    frame     TEXT NOT NULL,
    PRIMARY KEY (bundle_id, slot)
);

PRAGMA user_version = 3;
"#,
    ];

    /// A structural description of the whole schema, for comparing two
    /// databases. Covers every object in `sqlite_master` (tables, indexes,
    /// triggers, views) via its DDL text — normalized for whitespace and
    /// `IF NOT EXISTS` so equivalent spellings compare equal — which pins
    /// constraints (`CHECK`, foreign keys) that column listings alone would
    /// miss, plus each table's `PRAGMA table_info` structure.
    fn schema_snapshot(conn: &Connection) -> Vec<String> {
        let mut out: Vec<String> = conn
            .prepare(
                "SELECT type, name, sql FROM sqlite_master \
                 WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
            )
            .unwrap()
            .query_map([], |row| {
                let object_type: String = row.get(0)?;
                let name: String = row.get(1)?;
                // `sql` is NULL for auto-created objects; keep the entry so a
                // missing object still shows up as a difference.
                let ddl = row.get::<_, Option<String>>(2)?.map(|sql| {
                    sql.replace("IF NOT EXISTS ", "")
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                });
                Ok(format!("{object_type} {name}: {ddl:?}"))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();

        let tables: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master \
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        for table in tables {
            let mut stmt = conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap();
            let columns = stmt
                .query_map([], |row| {
                    Ok(format!(
                        "{table}.{}: {} notnull={} default={:?} pk={}",
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i32>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, i32>(5)?,
                    ))
                })
                .unwrap();
            out.extend(columns.map(|column| column.unwrap()));
        }
        out
    }

    #[test]
    fn opens_and_migrates_in_memory() {
        let db = Database::open_in_memory().expect("open");
        // Re-running migrate is idempotent.
        db.migrate().expect("re-migrate");
    }

    #[test]
    fn quick_check_accepts_a_clean_empty_database() {
        let conn = Connection::open_in_memory().expect("open");
        quick_check(&conn).expect("clean database");
    }

    #[test]
    fn persisted_state_preflight_distinguishes_settings_states() {
        let db = Database::open_in_memory().expect("open");

        let empty = db.preflight_persisted_state().expect("preflight empty");
        assert_eq!(empty.settings, PersistedSettings::Missing);
        assert_eq!(empty.settings_rows, PersistedRowCounts::default());
        assert!(empty.is_pristine());

        let settings = AppSettings {
            keyboard_enabled: false,
            ..AppSettings::default()
        };
        db.save_settings(&settings).expect("save settings");
        let ready = db.preflight_persisted_state().expect("preflight ready");
        assert_eq!(ready.settings, PersistedSettings::Ready(settings));
        assert_eq!(
            ready.settings_rows,
            PersistedRowCounts {
                stored: 1,
                skipped: 0,
            }
        );
        assert!(!ready.is_pristine());

        db.with_conn(|conn| {
            conn.execute("UPDATE settings SET data = 'not-json' WHERE id = 1", [])?;
            Ok(())
        })
        .expect("damage settings JSON");
        let unreadable = db
            .preflight_persisted_state()
            .expect("JSON damage is reported, not a SQL failure");
        assert!(matches!(
            unreadable.settings,
            PersistedSettings::UnreadableJson { ref message } if !message.is_empty()
        ));
        assert_eq!(
            unreadable.settings_rows,
            PersistedRowCounts {
                stored: 1,
                skipped: 1,
            }
        );
        assert!(!unreadable.is_pristine());
    }

    #[test]
    fn persisted_state_preflight_rejects_a_noncanonical_settings_id() {
        let db = Database::open_in_memory().expect("open");
        db.with_conn(|conn| {
            conn.execute_batch(
                r#"
                PRAGMA ignore_check_constraints = ON;
                INSERT INTO settings (id, data)
                VALUES (2, '{"launchAtLogin":false}');
                "#,
            )?;
            Ok(())
        })
        .expect("write noncanonical settings row");

        let error = db
            .preflight_persisted_state()
            .expect_err("a noncanonical settings id must be a hard failure");
        assert!(matches!(
            error,
            Error::Invalid {
                field: "settings.id",
                ..
            }
        ));
    }

    #[test]
    fn persisted_state_preflight_counts_skipped_keyboard_json_rows() {
        use crate::domain::action::AppAction;
        use crate::domain::keyboard::{KeySide, ModifierKey};
        use rusqlite::params;

        let db = Database::open_in_memory().expect("open");
        let hotkey = Hotkey {
            id: "good-hotkey".into(),
            label: "Good hotkey".into(),
            accelerator: "Cmd+Alt+1".into(),
            action: AppAction::TogglePanel,
            enabled: true,
        };
        let rule = ModifierRule {
            id: "good-rule".into(),
            label: "Good rule".into(),
            modifier: ModifierKey::CapsLock,
            side: KeySide::Either,
            remap_to: Some(ModifierKey::Control),
            hyper: false,
            tap: AppAction::NoOp,
            enabled: true,
        };
        db.upsert_hotkey(&hotkey).expect("write valid hotkey");
        db.upsert_modifier_rule(&rule).expect("write valid rule");
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO hotkeys (id, label, accelerator, action, enabled)
                 VALUES ('bad-hotkey', 'Bad hotkey', 'Cmd+Alt+2', 'not-json', 1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO modifier_rules
                   (id, label, modifier, side, remap_to, hyper, tap, enabled)
                 VALUES ('bad-rule', 'Bad rule', ?1, ?2, NULL, 0, 'not-json', 1)",
                params![
                    serde_json::to_string(&ModifierKey::CapsLock)?,
                    serde_json::to_string(&KeySide::Either)?,
                ],
            )?;
            Ok(())
        })
        .expect("write malformed JSON rows");

        let report = db.preflight_persisted_state().expect("preflight");
        assert_eq!(report.hotkeys, vec![hotkey]);
        assert_eq!(report.modifier_rules, vec![rule]);
        assert_eq!(
            report.hotkey_rows,
            PersistedRowCounts {
                stored: 2,
                skipped: 1,
            }
        );
        assert_eq!(report.hotkey_rows, report.modifier_rule_rows);
        assert!(
            !report.is_pristine(),
            "stored rows that were skipped must still prevent first-run seeding"
        );
    }

    #[test]
    fn persisted_state_preflight_counts_meta_and_placements_as_data() {
        use crate::domain::{NormalizedRect, PlacementSlot, WindowApplication, WindowPlacement};

        let db = Database::open_in_memory().expect("open");
        db.set_meta("permission_snapshot", "stored")
            .expect("write metadata");
        db.save_window_placement(&WindowPlacement {
            application: WindowApplication {
                bundle_id: "com.example.Editor".into(),
                name: "Editor".into(),
            },
            slot: PlacementSlot::Primary,
            frame: NormalizedRect::new(0.0, 0.0, 0.5, 1.0),
        })
        .expect("write placement");

        let report = db.preflight_persisted_state().expect("preflight");
        assert_eq!(
            report.meta_rows,
            PersistedRowCounts {
                stored: 1,
                skipped: 0,
            }
        );
        assert_eq!(report.meta_rows, report.window_placement_rows);
        assert!(!report.is_pristine());
    }

    #[test]
    fn persisted_state_preflight_requires_meta_and_placement_schema() {
        let mutations = [
            "DROP TABLE meta",
            "ALTER TABLE meta RENAME COLUMN value TO missing_value",
            "DROP TABLE window_placements",
            "ALTER TABLE window_placements RENAME COLUMN frame TO missing_frame",
        ];
        for mutation in mutations {
            let db = Database::open_in_memory().expect("open");
            db.with_conn(|conn| {
                conn.execute_batch(mutation)?;
                Ok(())
            })
            .expect("mutate schema");

            let error = db
                .preflight_persisted_state()
                .expect_err("a missing table or required column must be a hard failure");
            assert!(matches!(&error, Error::Database(_)), "{mutation}: {error}");
        }
    }

    #[test]
    fn persisted_state_preflight_rejects_scalar_type_damage() {
        let damaged_rows = [
            "INSERT INTO settings (id, data) VALUES (1, x'80')",
            r#"INSERT INTO hotkeys (id, label, accelerator, action, enabled)
               VALUES ('bad', 'Bad', 'Cmd+1', '{"type":"togglePanel"}', x'80')"#,
            r#"INSERT INTO modifier_rules
                 (id, label, modifier, side, remap_to, hyper, tap, enabled)
               VALUES
                 ('bad', 'Bad', '"capsLock"', '"either"', NULL, x'80',
                  '{"type":"noOp"}', 1)"#,
            "INSERT INTO meta (key, value) VALUES ('bad', x'80')",
            r#"INSERT INTO window_placements (bundle_id, app_name, slot, frame)
               VALUES ('com.example.Bad', x'80', 'primary',
                       '{"x":0.0,"y":0.0,"width":1.0,"height":1.0}')"#,
        ];
        for sql in damaged_rows {
            let db = Database::open_in_memory().expect("open");
            db.with_conn(|conn| {
                conn.execute_batch(sql)?;
                Ok(())
            })
            .expect("write scalar-damaged row");

            let error = db
                .preflight_persisted_state()
                .expect_err("a scalar conversion failure must abort preflight");
            assert!(matches!(&error, Error::Database(_)), "{sql}: {error}");
        }
    }

    #[test]
    fn persisted_state_preflight_skips_only_damaged_placement_payloads() {
        let db = Database::open_in_memory().expect("open");
        db.with_conn(|conn| {
            conn.execute_batch(
                r#"
                PRAGMA ignore_check_constraints = ON;
                INSERT INTO window_placements (bundle_id, app_name, slot, frame) VALUES
                  ('com.example.Good', 'Good', 'primary',
                   '{"x":0.0,"y":0.0,"width":0.5,"height":1.0}'),
                  ('com.example.Json', 'JSON', 'primary', 'not-json'),
                  ('com.example.Bounds', 'Bounds', 'primary',
                   '{"x":0.8,"y":0.0,"width":0.5,"height":1.0}'),
                  ('com.example.Slot', 'Slot', 'tertiary',
                   '{"x":0.0,"y":0.0,"width":1.0,"height":1.0}');
                "#,
            )?;
            Ok(())
        })
        .expect("write placement fixtures");

        let report = db
            .preflight_persisted_state()
            .expect("placement payload damage is row-local");
        assert_eq!(
            report.window_placement_rows,
            PersistedRowCounts {
                stored: 4,
                skipped: 3,
            }
        );
        assert!(!report.is_pristine());
    }

    #[test]
    fn migrated_schema_without_rows_has_no_persisted_data() {
        let db = Database::open_in_memory().expect("open");

        assert!(!db.has_persisted_data().expect("probe persisted data"));
    }

    #[test]
    fn persisted_data_probe_includes_internal_metadata() {
        let db = Database::open_in_memory().expect("open");
        db.set_meta("permission_snapshot", "stored")
            .expect("write metadata");

        assert!(db.has_persisted_data().expect("probe persisted data"));
    }

    #[test]
    fn persisted_data_probe_propagates_read_failures() {
        let db = Database::open_in_memory().expect("open");
        db.with_conn(|conn| {
            conn.execute("DROP TABLE window_placements", [])?;
            Ok(())
        })
        .expect("damage schema");

        db.has_persisted_data()
            .expect_err("an unreadable persistent table must fail the probe");
    }

    #[test]
    fn fresh_database_has_the_full_schema() {
        // The full migration chain must create every table along with the
        // `hyper` column. Each list query touches those columns, so a dropped
        // column or table would surface here.
        let db = Database::open_in_memory().expect("open");
        assert!(db.list_hotkeys().expect("hotkeys").is_empty());
        assert!(db.list_modifier_rules().expect("modifier rules").is_empty());
        assert!(
            db.list_window_placements("com.example.Empty")
                .expect("window placements")
                .is_empty()
        );
    }

    #[test]
    fn refuses_to_open_a_database_from_a_newer_schema_version() {
        // A database stamped with a `user_version` ahead of what this binary
        // knows about was written by a newer app version. Opening it as if it
        // matched the current schema could silently corrupt or drop data, so
        // it must be rejected instead.
        let db = Database::open_in_memory().expect("open");
        db.with_conn(|conn| {
            conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)?;
            Ok(())
        })
        .expect("bump user_version");

        let err = db.migrate().expect_err("newer schema must be rejected");
        assert!(matches!(err, Error::Migration(_)));
    }

    #[test]
    fn applies_pending_migrations_stepwise_and_resumes() {
        // A database part-way through a synthetic migration history picks up
        // exactly the steps it has not seen, stamping the version as it goes.
        let migrations: &[&str] = &["CREATE TABLE a (x INTEGER);", "CREATE TABLE b (y INTEGER);"];
        let conn = Connection::open_in_memory().expect("open");

        apply_migrations(&conn, &migrations[..1]).expect("first step");
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1);

        apply_migrations(&conn, migrations).expect("remaining steps");
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 2);
        // Both steps' tables exist; re-running is a no-op.
        conn.execute("INSERT INTO a (x) VALUES (1)", []).unwrap();
        conn.execute("INSERT INTO b (y) VALUES (1)", []).unwrap();
        apply_migrations(&conn, migrations).expect("idempotent");
    }

    #[test]
    fn a_failing_migration_step_rolls_back_and_keeps_the_version_reached() {
        // A step that fails part-way through must leave no trace of itself —
        // neither its tables nor its version stamp — while the steps before it
        // stay applied, so the next launch resumes from the failure point.
        let migrations: &[&str] = &[
            "CREATE TABLE a (x INTEGER);",
            "CREATE TABLE b (y INTEGER); THIS IS NOT SQL;",
        ];
        let conn = Connection::open_in_memory().expect("open");

        let err = apply_migrations(&conn, migrations).expect_err("second step must fail");
        assert!(matches!(err, Error::MigrationStep { step: 2, .. }));

        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1, "only the successful step is stamped");
        conn.execute("INSERT INTO a (x) VALUES (1)", [])
            .expect("step 1's table survives");
        assert!(
            conn.execute("INSERT INTO b (y) VALUES (1)", []).is_err(),
            "the failed step's table was rolled back"
        );
    }

    #[test]
    fn upgrades_a_database_from_every_past_schema_version() {
        // For each version a released build may have left on disk, recreate a
        // database frozen at that version from its snapshot, reopen it through
        // the normal path and verify it reaches the latest schema with every
        // table usable. Includes version 0 (an empty database) and the latest
        // itself (a no-op open); new snapshots extend the loop automatically.
        for version in 0..=VERSION_FIXTURES.len() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("tomari.sqlite3");

            let conn = Connection::open(&path).expect("open raw");
            if version > 0 {
                conn.execute_batch(VERSION_FIXTURES[version - 1])
                    .expect("build fixture");
            }
            drop(conn);

            let db = Database::open(&path).expect("upgrade to latest");
            let reached = db
                .with_conn(|conn| {
                    Ok(conn.pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))?)
                })
                .unwrap();
            assert_eq!(reached, SCHEMA_VERSION, "from version {version}");
            assert!(db.list_hotkeys().expect("hotkeys").is_empty());
            assert!(db.list_modifier_rules().expect("rules").is_empty());
            assert!(!db.settings_exist().expect("settings probe"));
            assert_eq!(db.get_meta("probe").expect("meta probe"), None);
            assert!(
                db.list_window_placements("com.example.Empty")
                    .expect("window placements")
                    .is_empty()
            );
        }
    }

    #[test]
    fn a_failed_migration_step_keeps_the_sqlite_error() {
        let conn = Connection::open_in_memory().expect("open");
        let err = apply_migrations(&conn, &["CREATE TABLE ok (id INTEGER)", "THIS IS NOT SQL"])
            .expect_err("the second step must fail");
        match &err {
            Error::MigrationStep { step, source } => {
                assert_eq!(*step, 2);
                // The SQLite error travels intact (here a parse failure, which
                // rusqlite reports as `SqlInputError`; a real corruption would
                // be a `SqliteFailure` with its code).
                assert!(
                    matches!(source, rusqlite::Error::SqlInputError { .. }),
                    "{source:?}"
                );
            }
            other => panic!("expected MigrationStep, got {other:?}"),
        }
        assert!(!err.is_database_corruption());
        // The first step stayed committed; the failed one rolled back.
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1);
    }

    #[test]
    fn a_past_schema_fixture_corrupted_on_disk_is_recognised_as_corruption() {
        // A v1 database whose first page (schema + table roots) is trashed
        // past the header: the file still *looks* like SQLite, so opening it
        // succeeds and the damage surfaces only once the migration reads the
        // schema. That error must still classify as corruption so launch
        // quarantines the file instead of failing the same way every time.
        use std::io::{Seek, SeekFrom, Write};
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tomari.sqlite3");
        {
            let conn = Connection::open(&path).expect("open raw");
            conn.execute_batch(VERSION_FIXTURES[0])
                .expect("build fixture");
        }
        {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .expect("reopen for damage");
            // Past the 100-byte header, over the rest of page 1 where the
            // schema cells live.
            file.seek(SeekFrom::Start(200)).unwrap();
            file.write_all(&vec![0xFF; 3_800]).unwrap();
        }
        let err = match Database::open(&path) {
            Ok(_) => panic!("a trashed schema page must not open"),
            Err(e) => e,
        };
        assert!(
            err.is_database_corruption(),
            "expected corruption, got {err:?}"
        );
    }

    #[test]
    fn latest_schema_damage_outside_page_one_is_recognised_as_corruption() {
        use std::io::{Seek, SeekFrom, Write};

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tomari.sqlite3");
        {
            let db = Database::open(&path).expect("create latest database");
            db.seed_defaults(&[], &[], &AppSettings::default())
                .expect("seed settings row");
        }

        let conn = Connection::open(&path).expect("open for checkpoint");
        let (busy, _, _): (i32, i32, i32) = conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .expect("checkpoint WAL");
        assert_eq!(busy, 0, "checkpoint must own the database");
        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
            .expect("leave no WAL sidecar");
        assert_eq!(journal_mode, "delete");
        let page_size: i64 = conn
            .pragma_query_value(None, "page_size", |row| row.get(0))
            .expect("page size");
        let (page_number, page_type): (i64, String) = conn
            .query_row(
                "SELECT pageno, pagetype FROM dbstat \
                 WHERE name = 'settings' AND path = '/' AND pageno <> 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("settings root page from dbstat");
        assert_eq!(page_type, "leaf");
        assert!(page_number > 1);
        assert!(page_size > 0);
        drop(conn);

        let offset = (page_number - 1)
            .checked_mul(page_size)
            .and_then(|offset| offset.try_into().ok())
            .expect("page offset fits u64");
        let damaged_page = vec![0xFF; page_size.try_into().expect("page size fits usize")];
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open database for damage");
        file.seek(SeekFrom::Start(offset)).expect("seek to page");
        file.write_all(&damaged_page).expect("damage one page");
        file.sync_all().expect("flush damage");
        drop(file);

        let error = match Database::open(&path) {
            Ok(_) => panic!("a damaged latest-schema table page must not open"),
            Err(error) => error,
        };
        assert!(
            matches!(&error, Error::DatabaseIntegrity(report) if !report.is_empty()),
            "quick_check should return its integrity diagnostic: {error:?}"
        );
        assert!(error.is_database_corruption());
    }

    #[test]
    fn migration_chain_produces_the_frozen_latest_schema() {
        // Running every migration from scratch must land on exactly the
        // structure captured in the latest frozen snapshot. This pins the two
        // definitions together: editing a shipped migration (or forgetting to
        // freeze a new version's snapshot) diverges them and fails here.
        let migrated = Connection::open_in_memory().expect("open");
        apply_migrations(&migrated, MIGRATIONS).expect("migrate");

        let frozen = Connection::open_in_memory().expect("open");
        frozen
            .execute_batch(VERSION_FIXTURES.last().expect("at least one version"))
            .expect("build fixture");

        assert_eq!(schema_snapshot(&migrated), schema_snapshot(&frozen));
        assert_eq!(MIGRATIONS.len(), VERSION_FIXTURES.len());
    }

    #[test]
    fn refuses_a_negative_schema_version() {
        // SQLite happily stores `PRAGMA user_version = -1`; treating it as "no
        // migrations applied" would index the migration list out of bounds and
        // panic, which the release profile turns into a silent exit.
        let conn = Connection::open_in_memory().expect("open");
        conn.pragma_update(None, "user_version", -1).unwrap();

        let err = apply_migrations(&conn, MIGRATIONS).expect_err("must refuse");
        assert!(matches!(err, Error::Migration(_)));
    }

    #[test]
    fn a_step_that_lost_a_launch_race_is_skipped_under_the_lock() {
        // Two instances can both read `user_version` before either has
        // migrated (the database opens before the single-instance guard
        // engages). Simulate the loser: it computed `next == 1` from that
        // stale read, but the winner has already committed step 1. The
        // re-check under the write lock must skip — re-running the DDL would
        // fail on the existing table.
        let migrations: &[&str] = &["CREATE TABLE a (x INTEGER);"];
        let conn = Connection::open_in_memory().expect("open");
        apply_migrations(&conn, migrations).expect("winner migrates");

        apply_step(&conn, migrations, 1).expect("loser skips instead of re-applying");
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1);
    }

    #[test]
    fn a_version_advanced_past_this_binary_is_refused_under_the_lock() {
        // The racing instance may be a *newer* binary that migrated beyond
        // what this one supports. The re-check under the lock must refuse —
        // treating "already past my target" as success would report a clean
        // launch against a schema this binary does not understand.
        let migrations: &[&str] = &["CREATE TABLE a (x INTEGER);"];
        let conn = Connection::open_in_memory().expect("open");
        conn.pragma_update(None, "user_version", 2).unwrap();

        let err = apply_step(&conn, migrations, 1).expect_err("must refuse");
        assert!(matches!(err, Error::Migration(_)));
    }

    #[test]
    fn a_concurrent_loser_on_its_own_connection_blocks_then_skips() {
        // The real race: a second connection calls `apply_step` while the
        // winner holds the write lock. The immediate transaction makes the
        // loser wait (busy timeout) for the winner's commit and then read the
        // stamped version; a deferred transaction would instead read a stale
        // snapshot and try to re-apply the DDL.
        use std::thread;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tomari.sqlite3");
        let migrations: &[&str] = &["CREATE TABLE a (x INTEGER);"];

        let winner = Connection::open(&path).expect("open winner");
        winner.pragma_update(None, "journal_mode", "WAL").unwrap();
        let tx = Transaction::new_unchecked(&winner, TransactionBehavior::Immediate)
            .expect("winner takes the write lock");

        let loser_path = path.clone();
        let loser = thread::spawn(move || {
            let conn = Connection::open(&loser_path).expect("open loser");
            conn.busy_timeout(Duration::from_secs(10)).unwrap();
            apply_step(&conn, &["CREATE TABLE a (x INTEGER);"], 1)
        });

        // Give the loser time to block on the lock, then finish the step and
        // release it. (If the loser only starts after the commit, it still
        // exercises the skip path — the test never becomes timing-flaky.)
        thread::sleep(Duration::from_millis(200));
        tx.execute_batch(migrations[0]).unwrap();
        tx.pragma_update(None, "user_version", 1).unwrap();
        tx.commit().unwrap();

        loser
            .join()
            .expect("loser thread")
            .expect("loser skips cleanly");
        let version: i32 = winner
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1);
    }

    #[test]
    fn seed_defaults_writes_every_row_and_marks_first_run() {
        use crate::domain::action::AppAction;
        use crate::domain::keyboard::{KeySide, ModifierKey};
        use crate::domain::window::WindowPreset;

        let db = Database::open_in_memory().expect("open");
        assert!(!db.settings_exist().unwrap(), "starts uninitialized");

        let hotkeys = vec![Hotkey {
            id: "h1".into(),
            label: "Snap left".into(),
            accelerator: "Cmd+Alt+Left".into(),
            action: AppAction::SnapWindow(WindowPreset::LeftHalf),
            enabled: true,
        }];
        let rules = vec![ModifierRule {
            id: "m1".into(),
            label: "Caps → Ctrl".into(),
            modifier: ModifierKey::CapsLock,
            side: KeySide::Either,
            remap_to: Some(ModifierKey::Control),
            hyper: false,
            tap: AppAction::SendKeystroke("Escape".into()),
            enabled: true,
        }];
        let settings = AppSettings::default();

        db.seed_defaults(&hotkeys, &rules, &settings).expect("seed");

        assert!(db.settings_exist().unwrap(), "settings row now present");
        assert_eq!(db.list_hotkeys().unwrap(), hotkeys);
        assert_eq!(db.list_modifier_rules().unwrap(), rules);
        assert_eq!(db.get_settings().unwrap(), settings);
    }

    #[test]
    fn seed_defaults_is_atomic_and_idempotent() {
        use crate::domain::action::AppAction;
        use crate::domain::window::WindowPreset;

        let db = Database::open_in_memory().expect("open");
        // An empty batch still writes the settings row (the first-run marker),
        // so a later launch correctly sees the database as initialized.
        db.seed_defaults(&[], &[], &AppSettings::default())
            .expect("seed empty");
        assert!(db.settings_exist().unwrap());
        assert!(db.list_hotkeys().unwrap().is_empty());

        // Re-seeding re-runs the same upserts by primary key, so it never
        // duplicates rows — the writes are keyed, not blindly appended.
        let hotkeys = vec![Hotkey {
            id: "h1".into(),
            label: "Snap left".into(),
            accelerator: "Cmd+Alt+Left".into(),
            action: AppAction::SnapWindow(WindowPreset::LeftHalf),
            enabled: true,
        }];
        db.seed_defaults(&hotkeys, &[], &AppSettings::default())
            .expect("re-seed");
        db.seed_defaults(&hotkeys, &[], &AppSettings::default())
            .expect("re-seed again");
        assert_eq!(db.list_hotkeys().unwrap(), hotkeys);
    }

    #[test]
    fn automation_reset_replaces_unreadable_rows_and_preserves_placements() {
        use crate::domain::{NormalizedRect, PlacementSlot, WindowApplication, WindowPlacement};

        let db = Database::open_in_memory().expect("open");
        let mut hotkeys = crate::defaults::default_hotkeys();
        let mut rules = crate::defaults::default_modifier_rules();
        db.seed_defaults(&hotkeys, &rules, &AppSettings::default())
            .expect("seed");
        let placement = WindowPlacement {
            application: WindowApplication {
                bundle_id: "com.example.Editor".into(),
                name: "Editor".into(),
            },
            slot: PlacementSlot::Primary,
            frame: NormalizedRect::new(0.1, 0.2, 0.6, 0.7),
        };
        db.save_window_placement(&placement)
            .expect("save placement");

        db.with_conn(|conn| {
            conn.execute("UPDATE hotkeys SET enabled = 'unreadable'", [])?;
            Ok(())
        })
        .expect("damage scalar type");
        assert!(
            db.list_hotkeys().is_err(),
            "fixture must break the list read"
        );

        let safe = AppSettings::fail_closed();
        db.reset_startup_configuration(
            StartupConfigurationReset::ReplaceAutomation,
            &hotkeys,
            &rules,
            &safe,
        )
        .expect("explicit reset");

        let mut stored_hotkeys = db.list_hotkeys().unwrap();
        stored_hotkeys.sort_by(|left, right| left.id.cmp(&right.id));
        hotkeys.sort_by(|left, right| left.id.cmp(&right.id));
        assert_eq!(stored_hotkeys, hotkeys);
        let mut stored_rules = db.list_modifier_rules().unwrap();
        stored_rules.sort_by(|left, right| left.id.cmp(&right.id));
        rules.sort_by(|left, right| left.id.cmp(&right.id));
        assert_eq!(stored_rules, rules);
        assert_eq!(db.get_settings().unwrap(), safe);
        assert_eq!(
            db.list_window_placements("com.example.Editor").unwrap(),
            vec![placement]
        );
    }

    #[test]
    fn automation_reset_rolls_back_every_delete_when_a_default_write_fails() {
        use crate::domain::action::AppAction;
        use crate::domain::window::WindowPreset;

        let db = Database::open_in_memory().expect("open");
        let original_hotkey = Hotkey {
            id: "custom".into(),
            label: "Custom".into(),
            accelerator: "Cmd+Alt+8".into(),
            action: AppAction::SnapWindow(WindowPreset::RightHalf),
            enabled: true,
        };
        let original_settings = AppSettings::fail_closed();
        db.seed_defaults(
            std::slice::from_ref(&original_hotkey),
            &[],
            &original_settings,
        )
        .expect("seed original");
        db.with_conn(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER reject_repair
                 BEFORE INSERT ON hotkeys
                 BEGIN
                   SELECT RAISE(ABORT, 'blocked repair');
                 END;",
            )?;
            Ok(())
        })
        .expect("install failure trigger");

        let error = db
            .reset_startup_configuration(
                StartupConfigurationReset::ReplaceAutomation,
                &crate::defaults::default_hotkeys(),
                &crate::defaults::default_modifier_rules(),
                &AppSettings::default(),
            )
            .expect_err("repair must fail");
        assert!(error.to_string().contains("blocked repair"));
        assert_eq!(db.list_hotkeys().unwrap(), vec![original_hotkey]);
        assert!(db.list_modifier_rules().unwrap().is_empty());
        assert_eq!(db.get_settings().unwrap(), original_settings);
    }

    #[test]
    fn on_disk_database_survives_close_and_reopen() {
        // In-memory tests never exercise the real file-backed path (WAL
        // checkpointing, the file actually persisting to disk). Open a file
        // under a temp directory, write through one connection, drop it, then
        // reopen the same path and confirm the data is still there.
        use crate::domain::action::AppAction;
        use crate::domain::keyboard::Hotkey;
        use crate::domain::window::WindowPreset;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tomari.sqlite3");

        let hk = Hotkey {
            id: "h1".into(),
            label: "Snap left".into(),
            accelerator: "Cmd+Alt+Left".into(),
            action: AppAction::SnapWindow(WindowPreset::LeftHalf),
            enabled: true,
        };

        {
            let db = Database::open(&path).expect("open on-disk db");
            db.upsert_hotkey(&hk).expect("write hotkey");
        } // `db` and its connection are dropped here.

        let db = Database::open(&path).expect("reopen on-disk db");
        assert_eq!(db.list_hotkeys().expect("hotkeys"), vec![hk]);
    }
}
