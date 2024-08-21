// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.

use deno_core::error::AnyError;
use deno_core::parking_lot::Mutex;
use deno_core::parking_lot::MutexGuard;
use deno_core::unsync::spawn_blocking;
use deno_webstorage::rusqlite;
use deno_webstorage::rusqlite::Connection;
use deno_webstorage::rusqlite::OptionalExtension;
use deno_webstorage::rusqlite::Params;
use once_cell::sync::OnceCell;
use std::io::IsTerminal;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use super::common::FastInsecureHasher;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct CacheDBHash(u64);

impl CacheDBHash {
    pub fn new(hash: u64) -> Self {
        Self(hash)
    }

    pub fn from_source(source: impl std::hash::Hash) -> Self {
        Self::new(
            // always write in the deno version just in case
            // the clearing on deno version change doesn't work
            FastInsecureHasher::new_deno_versioned()
                .write_hashable(source)
                .finish(),
        )
    }
}

impl rusqlite::types::ToSql for CacheDBHash {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(rusqlite::types::ToSqlOutput::Owned(
            // sqlite doesn't support u64, but it does support i64 so store
            // this value "incorrectly" as i64 then convert back to u64 on read
            rusqlite::types::Value::Integer(self.0 as i64),
        ))
    }
}

impl rusqlite::types::FromSql for CacheDBHash {
    fn column_result(value: rusqlite::types::ValueRef) -> rusqlite::types::FromSqlResult<Self> {
        match value {
            rusqlite::types::ValueRef::Integer(i) => Ok(Self::new(i as u64)),
            _ => Err(rusqlite::types::FromSqlError::InvalidType),
        }
    }
}

/// What should the cache should do on failure?
#[derive(Default)]
pub enum CacheFailure {
    /// Return errors if failure mode otherwise unspecified.
    #[default]
    Error,
    /// Create an in-memory cache that is not persistent.
    InMemory,
    /// Create a blackhole cache that ignores writes and returns empty reads.
    Blackhole,
}

/// Configuration SQL and other parameters for a [`CacheDB`].
pub struct CacheDBConfiguration {
    /// SQL to run for a new database.
    pub table_initializer: &'static str,
    /// SQL to run when the version from [`crate::version::deno()`] changes.
    pub on_version_change: &'static str,
    /// Prepared statements to pre-heat while initializing the database.
    pub preheat_queries: &'static [&'static str],
    /// What the cache should do on failure.
    pub on_failure: CacheFailure,
}

impl CacheDBConfiguration {
    fn create_combined_sql(&self) -> String {
        format!(
            concat!(
                "PRAGMA journal_mode=WAL;",
                "PRAGMA synchronous=NORMAL;",
                "PRAGMA temp_store=memory;",
                "PRAGMA page_size=4096;",
                "PRAGMA mmap_size=6000000;",
                "PRAGMA optimize;",
                "CREATE TABLE IF NOT EXISTS info (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
                "{}",
            ),
            self.table_initializer
        )
    }
}

enum ConnectionState {
    Connected(Connection),
    Blackhole,
    Error(Arc<AnyError>),
}

/// A cache database that eagerly initializes itself off-thread, preventing initialization operations
/// from blocking the main thread.
#[derive(Clone)]
pub struct CacheDB {
    // TODO(mmastrac): We can probably simplify our thread-safe implementation here
    conn: Arc<Mutex<OnceCell<ConnectionState>>>,
    path: Option<PathBuf>,
    config: &'static CacheDBConfiguration,
    version: &'static str,
}

impl Drop for CacheDB {
    fn drop(&mut self) {
        // No need to clean up an in-memory cache in an way -- just drop and go.
        let path = match self.path.take() {
            Some(path) => path,
            _ => return,
        };

        // If Deno is panicking, tokio is sometimes gone before we have a chance to shutdown. In
        // that case, we just allow the drop to happen as expected.
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }

        // For on-disk caches, see if we're the last holder of the Arc.
        let arc = std::mem::take(&mut self.conn);
        if let Ok(inner) = Arc::try_unwrap(arc) {
            // Hand off SQLite connection to another thread to do the surprisingly expensive cleanup
            let inner = inner.into_inner().into_inner();
            if let Some(conn) = inner {
                spawn_blocking(move || {
                    drop(conn);
                    log::trace!("Cleaned up SQLite connection at {}", path.to_string_lossy());
                });
            }
        }
    }
}

impl CacheDB {
    pub fn in_memory(config: &'static CacheDBConfiguration, version: &'static str) -> Self {
        CacheDB {
            conn: Arc::new(Mutex::new(OnceCell::new())),
            path: None,
            config,
            version,
        }
    }

    pub fn from_path(
        config: &'static CacheDBConfiguration,
        path: PathBuf,
        version: &'static str,
    ) -> Self {
        log::debug!("Opening cache {}...", path.to_string_lossy());
        let new = Self {
            conn: Arc::new(Mutex::new(OnceCell::new())),
            path: Some(path),
            config,
            version,
        };

        new.spawn_eager_init_thread();
        new
    }

    /// Useful for testing: re-create this cache DB with a different current version.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn recreate_with_version(mut self, version: &'static str) -> Self {
        // By taking the lock, we know there are no initialization threads alive
        drop(self.conn.lock());

        let arc = std::mem::take(&mut self.conn);
        let conn = match Arc::try_unwrap(arc) {
            Err(_) => panic!("Failed to unwrap connection"),
            Ok(conn) => match conn.into_inner().into_inner() {
                Some(ConnectionState::Connected(conn)) => conn,
                _ => panic!("Connection had failed and cannot be unwrapped"),
            },
        };

        Self::initialize_connection(self.config, &conn, version).unwrap();

        let cell = OnceCell::new();
        _ = cell.set(ConnectionState::Connected(conn));
        Self {
            conn: Arc::new(Mutex::new(cell)),
            path: self.path.clone(),
            config: self.config,
            version,
        }
    }

    fn spawn_eager_init_thread(&self) {
        let clone = self.clone();
        debug_assert!(tokio::runtime::Handle::try_current().is_ok());
        spawn_blocking(move || {
            let lock = clone.conn.lock();
            clone.initialize(&lock);
        });
    }

    /// Open the connection in memory or on disk.
    fn actually_open_connection(&self, path: Option<&Path>) -> Result<Connection, rusqlite::Error> {
        match path {
            // This should never fail unless something is very wrong
            None => Connection::open_in_memory(),
            Some(path) => Connection::open(path),
        }
    }

    /// Attempt to initialize that connection.
    fn initialize_connection(
        config: &CacheDBConfiguration,
        conn: &Connection,
        version: &str,
    ) -> Result<(), AnyError> {
        let sql = config.create_combined_sql();
        conn.execute_batch(&sql)?;

        // Check the version
        let existing_version = conn
            .query_row(
                "SELECT value FROM info WHERE key='CLI_VERSION' LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_default();

        // If Deno has been upgraded, run the SQL to update the version
        if existing_version != version {
            conn.execute_batch(config.on_version_change)?;
            let mut stmt =
                conn.prepare("INSERT OR REPLACE INTO info (key, value) VALUES (?1, ?2)")?;
            stmt.execute(["CLI_VERSION", version])?;
        }

        // Preheat any prepared queries
        for preheat in config.preheat_queries {
            drop(conn.prepare_cached(preheat)?);
        }
        Ok(())
    }

    /// Open and initialize a connection.
    fn open_connection_and_init(&self, path: Option<&Path>) -> Result<Connection, AnyError> {
        let conn = self.actually_open_connection(path)?;
        Self::initialize_connection(self.config, &conn, self.version)?;
        Ok(conn)
    }

    /// This function represents the policy for dealing with corrupted cache files. We try fairly aggressively
    /// to repair the situation, and if we can't, we prefer to log noisily and continue with in-memory caches.
    fn open_connection(&self) -> Result<ConnectionState, AnyError> {
        open_connection(self.config, self.path.as_deref(), |maybe_path| {
            self.open_connection_and_init(maybe_path)
        })
    }

    fn initialize<'a>(
        &self,
        lock: &'a MutexGuard<OnceCell<ConnectionState>>,
    ) -> &'a ConnectionState {
        lock.get_or_init(|| match self.open_connection() {
            Ok(conn) => conn,
            Err(e) => ConnectionState::Error(e.into()),
        })
    }

    pub fn with_connection<T: Default>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, AnyError>,
    ) -> Result<T, AnyError> {
        let lock = self.conn.lock();
        let conn = self.initialize(&lock);

        match conn {
            ConnectionState::Blackhole => {
                // Cache is a blackhole - nothing in or out.
                Ok(T::default())
            }
            ConnectionState::Error(e) => {
                // This isn't ideal because we lose the original underlying error
                let err = AnyError::msg(e.clone().to_string());
                Err(err)
            }
            ConnectionState::Connected(conn) => f(conn),
        }
    }

    #[cfg(test)]
    pub fn ensure_connected(&self) -> Result<(), AnyError> {
        self.with_connection(|_| Ok(()))
    }

    pub fn execute(&self, sql: &'static str, params: impl Params) -> Result<usize, AnyError> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare_cached(sql)?;
            let res = stmt.execute(params)?;
            Ok(res)
        })
    }

    pub fn exists(&self, sql: &'static str, params: impl Params) -> Result<bool, AnyError> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare_cached(sql)?;
            let res = stmt.exists(params)?;
            Ok(res)
        })
    }

    /// Query a row from the database with a mapping function.
    pub fn query_row<T, F>(
        &self,
        sql: &'static str,
        params: impl Params,
        f: F,
    ) -> Result<Option<T>, AnyError>
    where
        F: FnOnce(&rusqlite::Row<'_>) -> Result<T, AnyError>,
    {
        let res = self.with_connection(|conn| {
            let mut stmt = conn.prepare_cached(sql)?;
            let mut rows = stmt.query(params)?;
            if let Some(row) = rows.next()? {
                let res = f(row)?;
                Ok(Some(res))
            } else {
                Ok(None)
            }
        })?;
        Ok(res)
    }
}

/// This function represents the policy for dealing with corrupted cache files. We try fairly aggressively
/// to repair the situation, and if we can't, we prefer to log noisily and continue with in-memory caches.
fn open_connection(
    config: &CacheDBConfiguration,
    path: Option<&Path>,
    open_connection_and_init: impl Fn(Option<&Path>) -> Result<Connection, AnyError>,
) -> Result<ConnectionState, AnyError> {
    // Success on first try? We hope that this is the case.
    let err = match open_connection_and_init(path) {
        Ok(conn) => return Ok(ConnectionState::Connected(conn)),
        Err(err) => err,
    };

    let Some(path) = path.as_ref() else {
        // If an in-memory DB fails, that's game over
        log::error!("Failed to initialize in-memory cache database.");
        return Err(err);
    };

    // ensure the parent directory exists
    if let Some(parent) = path.parent() {
        match std::fs::create_dir_all(parent) {
            Ok(_) => {
                log::debug!("Created parent directory for cache db.");
            }
            Err(err) => {
                log::debug!("Failed creating the cache db parent dir: {:#}", err);
            }
        }
    }

    // There are rare times in the tests when we can't initialize a cache DB the first time, but it succeeds the second time, so
    // we don't log these at a debug level.
    log::trace!(
        "Could not initialize cache database '{}', retrying... ({err:?})",
        path.to_string_lossy(),
    );

    // Try a second time
    let err = match open_connection_and_init(Some(path)) {
        Ok(conn) => return Ok(ConnectionState::Connected(conn)),
        Err(err) => err,
    };

    // Failed, try deleting it
    let is_tty = std::io::stderr().is_terminal();
    log::log!(
        if is_tty {
            log::Level::Warn
        } else {
            log::Level::Trace
        },
        "Could not initialize cache database '{}', deleting and retrying... ({err:?})",
        path.to_string_lossy()
    );
    if std::fs::remove_file(path).is_ok() {
        // Try a third time if we successfully deleted it
        let res = open_connection_and_init(Some(path));
        if let Ok(conn) = res {
            return Ok(ConnectionState::Connected(conn));
        };
    }

    match config.on_failure {
        CacheFailure::InMemory => {
            log::log!(
                if is_tty {
                    log::Level::Error
                } else {
                    log::Level::Trace
                },
                "Failed to open cache file '{}', opening in-memory cache.",
                path.to_string_lossy()
            );
            Ok(ConnectionState::Connected(open_connection_and_init(None)?))
        }
        CacheFailure::Blackhole => {
            log::log!(
                if is_tty {
                    log::Level::Error
                } else {
                    log::Level::Trace
                },
                "Failed to open cache file '{}', performance may be degraded.",
                path.to_string_lossy()
            );
            Ok(ConnectionState::Blackhole)
        }
        CacheFailure::Error => {
            log::error!(
                "Failed to open cache file '{}', expect further errors.",
                path.to_string_lossy()
            );
            Err(err)
        }
    }
}
