//! Disposable database support for integration tests.
//!
//! Each test creates its own database rather than sharing one: the behaviors
//! worth testing here — constraint refusal, idempotent re-application,
//! catalog shape — need a database whose contents no other test has touched.

use sqlx::Executor as _;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

use std::collections::{BTreeMap, VecDeque};
use std::io::{Cursor, Write as _};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use secrecy::SecretString;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::provider::{
    ExchangedToken, InstagramProvider, OAuthCodeRelay, ProviderAccount, ProviderError,
    ProviderFailureClass, ProviderFuture, ProviderOwnMediaPage, ProviderPermissions, RelayClaim,
    RelayError, RelayFuture,
};

use crate::{Database, PersistenceError};

/// Builds deterministic ZIP bytes from the supplied ordered entries.
///
/// The helper deliberately preserves input order and duplicate names so hostile
/// archive tests can express both. ZIP timestamps remain at the crate's fixed
/// DOS epoch default; repeated calls with identical input produce identical
/// bytes.
///
/// # Errors
///
/// Returns [`zip::result::ZipError`] when the in-memory archive cannot be
/// written.
pub fn data_export_zip(
    entries: &[(&str, &[u8])],
    compression: CompressionMethod,
) -> Result<Vec<u8>, zip::result::ZipError> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(compression);
    for (name, body) in entries {
        writer.start_file(*name, options)?;
        writer.write_all(body)?;
    }
    Ok(writer.finish()?.into_inner())
}

/// Builds a deterministic deflated ZIP containing the synthetic saved-post fixture.
///
/// # Errors
///
/// Returns [`zip::result::ZipError`] when the in-memory archive cannot be
/// written.
pub fn synthetic_saved_posts_export_zip() -> Result<Vec<u8>, zip::result::ZipError> {
    data_export_zip(
        &[(
            "your_instagram_activity/saved/saved_posts.json",
            include_bytes!("../tests/fixtures/data_export/saved_posts.json"),
        )],
        CompressionMethod::Deflated,
    )
}

/// How many connections one test may hold. The suite runs several test
/// binaries at once and each test owns a database, so larger pools exhaust
/// the server's connection budget before they make anything faster.
const TEST_POOL_SIZE: u32 = 2;

/// Where disposable databases are created.
///
/// `INSTAGRAM_ARCHIVE_TEST_DATABASE_URL` overrides it; the default matches
/// `compose.yaml`, so `docker compose up -d` followed by `cargo test` works
/// with no further setup.
///
/// # Panics
///
/// Never in normal operation; the environment read is the one sanctioned
/// exception to the closed-config rule because it names where tests may
/// create databases, which is not process configuration at all.
#[must_use]
#[expect(
    clippy::disallowed_methods,
    reason = "test-only database location is not process configuration"
)]
pub fn admin_url() -> String {
    match std::env::var("INSTAGRAM_ARCHIVE_TEST_DATABASE_URL") {
        Ok(value) => value,
        Err(_) => "postgres://instagram:instagram@127.0.0.1:5436/instagram".to_owned(),
    }
}

/// An isolated disposable archive database.
#[derive(Debug)]
pub struct TestDatabase {
    /// Connected archive database, ready for queries.
    pub database: Database,
    name: String,
}

impl TestDatabase {
    /// Creates an isolated database and applies the current schema definition.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] when database creation or application
    /// fails. A missing server is a real failure, never a skip.
    pub async fn create() -> Result<Self, PersistenceError> {
        let database = Self::create_raw().await?;
        database.database.apply_schema().await?;
        Ok(database)
    }

    /// Creates an isolated database WITHOUT applying the schema, for tests
    /// that drive application themselves (idempotency, concurrency).
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] when database creation fails.
    pub async fn create_raw() -> Result<Self, PersistenceError> {
        let name = format!("instagram_archive_test_{}", Uuid::now_v7().simple());
        let admin_url = admin_url();
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
            .map_err(PersistenceError::Connect)?;
        // The name is generated from a UUID, so it cannot carry an injection;
        // PostgreSQL has no bind parameters for identifiers in DDL.
        //
        // The locale is stated rather than inherited from template1, whose
        // collation is a property of whatever cluster happened to start:
        // ICU here matches compose.yaml, CI, and every other repository in
        // the fleet that checks text ordering against this one.
        admin
            .execute(
                format!(
                    r#"create database "{name}" template template0
                       locale_provider icu icu_locale 'und-x-icu' encoding 'UTF8'"#
                )
                .as_str(),
            )
            .await
            .map_err(PersistenceError::Query)?;
        admin.close().await;

        let options = admin_url
            .parse::<PgConnectOptions>()
            .map_err(PersistenceError::Connect)?
            .database(&name);
        let pool = PgPoolOptions::new()
            .max_connections(TEST_POOL_SIZE)
            .connect_with(options)
            .await
            .map_err(PersistenceError::Connect)?;

        Ok(Self {
            database: Database::from_pool(pool),
            name,
        })
    }

    /// The generated database name, for assertions about existence.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Closes the pool and drops the database.
    ///
    /// Explicit rather than a `Drop` impl: dropping requires async work, and
    /// a blocking drop inside a Tokio worker deadlocks. A test that panics
    /// leaves its database behind on purpose while the failure is read.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] when cleanup fails.
    pub async fn cleanup(self) -> Result<(), PersistenceError> {
        self.database.close().await;
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url())
            .await
            .map_err(PersistenceError::Connect)?;
        admin
            .execute(format!(r#"drop database if exists "{}" with (force)"#, self.name).as_str())
            .await
            .map_err(PersistenceError::Query)?;
        admin.close().await;
        Ok(())
    }
}

/// One scripted result consumed by [`FakeInstagramProvider`].
#[derive(Debug)]
pub enum FakeProviderStep {
    /// Authorization-code exchange result.
    Exchange(Result<ExchangedToken, ProviderError>),
    /// Account-discovery result.
    Account(Result<ProviderAccount, ProviderError>),
    /// Permission-discovery result.
    Permissions(Result<ProviderPermissions, ProviderError>),
    /// Refresh result.
    Refresh(Result<ExchangedToken, ProviderError>),
    /// Provider-side revoke result.
    Revoke(Result<(), ProviderError>),
    /// Connected-account own-media page result.
    OwnMedia(Result<ProviderOwnMediaPage, ProviderError>),
}

/// Deterministic no-network official provider.
#[derive(Debug, Clone)]
pub struct FakeInstagramProvider {
    steps: Arc<Mutex<VecDeque<FakeProviderStep>>>,
    calls: Arc<AtomicUsize>,
    own_media_cursors: Arc<Mutex<Vec<Option<String>>>>,
}

impl FakeInstagramProvider {
    /// Creates a fake that consumes exactly the supplied order.
    #[must_use]
    pub fn new(steps: impl IntoIterator<Item = FakeProviderStep>) -> Self {
        Self {
            steps: Arc::new(Mutex::new(steps.into_iter().collect())),
            calls: Arc::new(AtomicUsize::new(0)),
            own_media_cursors: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn next(&self) -> Result<FakeProviderStep, ProviderError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.steps
            .lock()
            .map_err(|_| fake_provider_error())?
            .pop_front()
            .ok_or_else(fake_provider_error)
    }

    /// Number of scripted calls not yet consumed.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.steps.lock().map_or(usize::MAX, |steps| steps.len())
    }

    /// Number of scripted provider calls attempted.
    #[must_use]
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }

    /// Own-media continuation values observed by scripted calls.
    #[must_use]
    pub fn own_media_cursors(&self) -> Vec<Option<String>> {
        self.own_media_cursors
            .lock()
            .map_or_else(|_| Vec::new(), |cursors| cursors.clone())
    }
}

impl InstagramProvider for FakeInstagramProvider {
    fn exchange_code<'a>(&'a self, _code: &'a SecretString) -> ProviderFuture<'a, ExchangedToken> {
        Box::pin(async move {
            match self.next()? {
                FakeProviderStep::Exchange(result) => result,
                _ => Err(fake_provider_error()),
            }
        })
    }

    fn discover_account<'a>(
        &'a self,
        _access_token: &'a SecretString,
    ) -> ProviderFuture<'a, ProviderAccount> {
        Box::pin(async move {
            match self.next()? {
                FakeProviderStep::Account(result) => result,
                _ => Err(fake_provider_error()),
            }
        })
    }

    fn discover_permissions<'a>(
        &'a self,
        _access_token: &'a SecretString,
    ) -> ProviderFuture<'a, ProviderPermissions> {
        Box::pin(async move {
            match self.next()? {
                FakeProviderStep::Permissions(result) => result,
                _ => Err(fake_provider_error()),
            }
        })
    }

    fn refresh_token<'a>(
        &'a self,
        _access_token: &'a SecretString,
    ) -> ProviderFuture<'a, ExchangedToken> {
        Box::pin(async move {
            match self.next()? {
                FakeProviderStep::Refresh(result) => result,
                _ => Err(fake_provider_error()),
            }
        })
    }

    fn revoke_token<'a>(&'a self, _access_token: &'a SecretString) -> ProviderFuture<'a, ()> {
        Box::pin(async move {
            match self.next()? {
                FakeProviderStep::Revoke(result) => result,
                _ => Err(fake_provider_error()),
            }
        })
    }

    fn list_own_media_page<'a>(
        &'a self,
        _provider_account_id: &'a str,
        _access_token: &'a SecretString,
        after: Option<&'a str>,
    ) -> ProviderFuture<'a, ProviderOwnMediaPage> {
        Box::pin(async move {
            self.own_media_cursors
                .lock()
                .map_err(|_| fake_provider_error())?
                .push(after.map(str::to_owned));
            match self.next()? {
                FakeProviderStep::OwnMedia(result) => result,
                _ => Err(fake_provider_error()),
            }
        })
    }
}

fn fake_provider_error() -> ProviderError {
    ProviderError {
        class: ProviderFailureClass::ResponseRefused,
        http_status: None,
    }
}

/// Deterministic one-time callback relay.
#[derive(Debug, Clone, Default)]
pub struct FakeOAuthCodeRelay {
    claims: Arc<Mutex<BTreeMap<String, RelayClaim>>>,
}

impl FakeOAuthCodeRelay {
    /// Adds a single-use relay claim.
    ///
    /// # Errors
    ///
    /// Returns [`RelayError::Transport`] if the test script lock is poisoned.
    pub fn insert(&self, relay_id: String, claim: RelayClaim) -> Result<(), RelayError> {
        self.claims
            .lock()
            .map_err(|_| RelayError::Transport)?
            .insert(relay_id, claim);
        Ok(())
    }
}

impl OAuthCodeRelay for FakeOAuthCodeRelay {
    fn claim<'a>(&'a self, relay_id: &'a str) -> RelayFuture<'a> {
        Box::pin(async move {
            self.claims
                .lock()
                .map_err(|_| RelayError::Transport)?
                .remove(relay_id)
                .ok_or(RelayError::Unavailable)
        })
    }
}
