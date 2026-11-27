//! Catalog refresh and runtime source selection.

use std::sync::Arc;
use std::time::SystemTime;

use crate::DblError;
use crate::catalog::select_artifact;
use crate::error::Result;
use crate::model::{EngineKey, InstanceSpec};
use crate::service::{OperationId, OperationStage, ServiceEvent};

use super::helpers::{emit_stage, finish_operation};
use super::runtime_lock::runtime_id;
use super::service::{Fetcher, Inner, RuntimeSource};

/// Local newtype so the adapter type is nameable from this module.
#[derive(Clone)]
pub(crate) struct FetcherAdapterFor(pub(crate) Arc<dyn Fetcher>);

impl crate::catalog::CatalogFetcher for FetcherAdapterFor {
    fn fetch(
        &self,
        url: &str,
        etag: Option<&str>,
    ) -> impl std::future::Future<Output = Result<crate::catalog::RefreshOutcome>> + Send {
        let url = url.to_owned();
        let etag = etag.map(str::to_owned);
        let inner = Arc::clone(&self.0);
        async move { inner.fetch_boxed(url, etag).await }
    }
}

pub(crate) fn refresh(inner: &Arc<Inner>, force: bool) -> Result<OperationId> {
    let op = inner.ops.next_id();
    let inner = Arc::clone(inner);
    let op_for_task = op.clone();
    inner.handle().spawn(async move {
        emit_stage(&inner, &op_for_task, OperationStage::Queued);
        let result = run_refresh(&inner, force).await;
        finish_operation(&inner, &op_for_task, result);
    });
    Ok(op)
}

async fn run_refresh(inner: &Arc<Inner>, force: bool) -> super::helpers::OpResult<()> {
    let state = inner
        .catalog
        .lock()
        .map_err(|_| DblError::Storage {
            reason: "the catalog lock was poisoned".to_owned(),
        })?
        .clone();
    let adapter = FetcherAdapterFor(Arc::clone(&inner.fetcher));
    let refreshed = crate::catalog::refresh(
        &state,
        &inner.paths,
        &adapter,
        force,
        SystemTime::now(),
        crate::catalog::catalog_ttl(),
    )
    .await?;
    let status = refreshed.status();
    if let Ok(mut catalog) = inner.catalog.lock() {
        *catalog = refreshed;
    }
    inner.emit(ServiceEvent::CatalogUpdated { status });
    Ok(())
}

/// Refuses an engine that ships no artifact at all for this `(os, arch)`.
///
/// The catalog is the single source of truth for "can this engine run here",
/// so the answer is data-driven rather than a `cfg!(windows)` list (D-26:
/// Valkey has Linux tarballs only, so Windows has no Valkey). An injected
/// runtime source is an explicit test seam and is treated as supported, so the
/// download path can be exercised for an engine the test supplies itself.
///
/// This runs before any store write or spawn, so a user on an unsupported
/// platform gets one typed `DBL-6001` and nothing is downloaded or started.
pub(crate) fn ensure_engine_supported(inner: &Inner, engine: EngineKey) -> Result<()> {
    if inner
        .runtime_sources
        .iter()
        .any(|source| source.engine == engine)
    {
        return Ok(());
    }
    let state = inner
        .catalog
        .lock()
        .map_err(|_| DblError::Storage {
            reason: "the catalog lock was poisoned".to_owned(),
        })?
        .clone();
    let supported = state
        .snapshot()
        .artifacts_for(inner.os, inner.arch)
        .iter()
        .any(|artifact| artifact.engine() == engine);
    if supported {
        return Ok(());
    }
    Err(DblError::Unsupported {
        reason: format!(
            "{} has no build for {} {}",
            engine.display_name(),
            inner.os,
            inner.arch
        ),
    })
}

/// Picks the runtime source for `spec`.
///
/// Sources injected via options win, which is how tests exercise the real
/// download/install pipeline offline. Otherwise the catalog snapshot is filtered
/// for this platform and the best glibc-compatible build is chosen.
pub(crate) fn source_for(inner: &Inner, spec: &InstanceSpec) -> Result<RuntimeSource> {
    if let Some(source) = inner
        .runtime_sources
        .iter()
        .find(|source| source.engine == spec.engine() && source.version == spec.version())
    {
        return Ok(source.clone());
    }

    // A version can be missing from a catalog that still lists the engine on
    // this platform (a retired or not-yet-added tag); that is also unsupported.
    ensure_engine_supported(inner, spec.engine())?;

    let state = inner
        .catalog
        .lock()
        .map_err(|_| DblError::Storage {
            reason: "the catalog lock was poisoned".to_owned(),
        })?
        .clone();
    let available = state.snapshot().artifacts_for(inner.os, inner.arch);
    let artifact = select_artifact(&available, spec.engine(), spec.version(), inner.host_glibc)
        .ok_or_else(|| DblError::Unsupported {
            reason: format!(
                "{} {} is not available on this platform",
                spec.engine().display_name(),
                spec.version()
            ),
        })?;

    Ok(RuntimeSource {
        id: artifact.id().to_owned(),
        url: artifact.url().to_owned(),
        sha256: artifact.sha256().to_owned(),
        size_bytes: artifact.size_bytes(),
        format: artifact.format(),
        strip_prefix: artifact.strip_prefix().map(str::to_owned),
        engine: artifact.engine(),
        version: artifact.version().to_owned(),
        expected_binaries: Vec::new(),
    })
}

/// Whether the runtime for this engine and version is already unpacked here.
///
/// The check asks the engine layout, never a fixed `bin/` path: RustFS unpacks
/// its single `rustfs` binary at the runtime root, so a `bin/` test made every
/// second RustFS create re-download and then collide with the existing
/// directory. It requires the same files extraction verified, so an incomplete
/// tree is not treated as installed — and because extraction refuses to write
/// over an existing final directory (`DBL-1003`), the installer then refuses to
/// overwrite it rather than silently repairing it. The app never produces such a
/// tree: extraction verifies the expected binaries in a staging directory and
/// only then renames it into place.
pub(crate) fn runtime_is_installed(inner: &Inner, spec: &InstanceSpec) -> bool {
    let Ok(dir) = inner
        .paths
        .runtime_dir(spec.engine(), spec.version(), inner.arch)
    else {
        return false;
    };
    crate::engine::layout::runtime_is_installed(&dir, spec.engine())
}

/// Records an installed runtime if the store does not have it yet.
pub(crate) fn record_runtime(inner: &Arc<Inner>, spec: &InstanceSpec) -> Result<()> {
    let spec = spec.clone();
    let inner = Arc::clone(inner);
    let dir = inner
        .paths
        .runtime_dir(spec.engine(), spec.version(), inner.arch)
        .map_err(|_| DblError::InvalidInput {
            field: "runtime",
            reason: "runtime path is invalid".to_owned(),
        })?;
    let id = runtime_id(spec.engine(), spec.version(), inner.arch);
    let engine = spec.engine();
    let version = spec.version().to_owned();
    let arch = inner.arch;
    let runtime = crate::model::Runtime::new(
        id.clone(),
        engine,
        version,
        arch,
        dir,
        0,
        SystemTime::now(),
        crate::model::RuntimeState::Installed,
    );
    inner.db(|store| match store.add_runtime(&runtime) {
        Ok(()) => Ok(()),
        Err(DblError::AlreadyExists { .. }) => Ok(()),
        Err(err) => Err(err),
    })
}
