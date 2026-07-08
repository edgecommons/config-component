//! CatalogSource seam and the v1 mounted-file implementations.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

const FILE_PROVENANCE_SOURCE: &str = "file";
const CONFIGMAP_PROVENANCE_SOURCE: &str = "configmap";
const ENV_PROVENANCE_SOURCE: &str = "env";
const DEFAULT_CONFIGMAP_MOUNT_DIR: &str = "/etc/edgecommons";
const DEFAULT_CONFIGMAP_CATALOG_KEY: &str = "catalog.json";
const DEFAULT_ENV_CATALOG_VAR: &str = "EDGECOMMONS_CONFIG_CATALOG";

/// A raw snapshot loaded from a catalog source.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceSnapshot {
    pub raw_catalog: Value,
    pub version: String,
    pub provenance: Map<String, Value>,
    pub fingerprint: String,
}

/// Pluggable catalog source contract.
pub trait CatalogSource: Send + Sync {
    fn load(&self) -> anyhow::Result<SourceSnapshot>;
    fn watch(&self) -> Option<mpsc::UnboundedReceiver<SourceSnapshot>>;
}

/// Local JSON catalog source with load and optional watch support.
#[derive(Debug, Clone)]
pub struct FileCatalogSource {
    path: PathBuf,
    watch_enabled: bool,
    poll_interval: Duration,
    provenance_source: &'static str,
}

impl FileCatalogSource {
    pub fn new(path: impl Into<PathBuf>, watch_enabled: bool) -> Self {
        Self::with_poll_interval(path, watch_enabled, Duration::from_secs(1))
    }

    pub fn with_poll_interval(
        path: impl Into<PathBuf>,
        watch_enabled: bool,
        poll_interval: Duration,
    ) -> Self {
        Self::with_provenance(path, watch_enabled, poll_interval, FILE_PROVENANCE_SOURCE)
    }

    fn with_provenance(
        path: impl Into<PathBuf>,
        watch_enabled: bool,
        poll_interval: Duration,
        provenance_source: &'static str,
    ) -> Self {
        Self {
            path: path.into(),
            watch_enabled,
            poll_interval,
            provenance_source,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl CatalogSource for FileCatalogSource {
    fn load(&self) -> anyhow::Result<SourceSnapshot> {
        let data = fs::read(&self.path)
            .with_context(|| format!("failed to read catalog file {}", self.path.display()))?;
        let raw_catalog: Value = serde_json::from_slice(&data)
            .with_context(|| format!("catalog file {} is not valid JSON", self.path.display()))?;
        if !raw_catalog.is_object() {
            bail!(
                "catalog file {} must contain a JSON object",
                self.path.display()
            );
        }
        let fingerprint = content_fingerprint(&data);
        let version = raw_catalog
            .get("version")
            .and_then(Value::as_str)
            .filter(|version| !version.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| fingerprint.clone());

        let provenance = raw_catalog
            .get("provenance")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_else(|| {
                let mut provenance = Map::new();
                provenance.insert(
                    "source".to_string(),
                    Value::String(self.provenance_source.to_string()),
                );
                provenance.insert(
                    "uri".to_string(),
                    Value::String(self.path.display().to_string()),
                );
                provenance.insert(
                    "contentHash".to_string(),
                    Value::String(fingerprint.clone()),
                );
                provenance
            });

        Ok(SourceSnapshot {
            raw_catalog,
            version,
            provenance,
            fingerprint,
        })
    }

    fn watch(&self) -> Option<mpsc::UnboundedReceiver<SourceSnapshot>> {
        if !self.watch_enabled {
            return None;
        }

        let source = self.clone();
        let mut last_fingerprint = source.load().ok().map(|snapshot| snapshot.fingerprint);
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(source.poll_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                match source.load() {
                    Ok(snapshot)
                        if Some(snapshot.fingerprint.as_str()) != last_fingerprint.as_deref() =>
                    {
                        last_fingerprint = Some(snapshot.fingerprint.clone());
                        if tx.send(snapshot).is_err() {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(error = %error, path = %source.path.display(), "catalog watch read failed");
                    }
                }
            }
        });
        Some(rx)
    }
}

/// Read-only Kubernetes ConfigMap catalog source.
///
/// This reads from a mounted ConfigMap key and may watch/poll that mounted file, but it never
/// writes back into the ConfigMap.
#[derive(Debug, Clone)]
pub struct ConfigMapCatalogSource {
    inner: FileCatalogSource,
}

impl ConfigMapCatalogSource {
    pub fn new(path: impl Into<PathBuf>, watch_enabled: bool) -> Self {
        Self::with_poll_interval(path, watch_enabled, Duration::from_secs(1))
    }

    pub fn with_poll_interval(
        path: impl Into<PathBuf>,
        watch_enabled: bool,
        poll_interval: Duration,
    ) -> Self {
        Self {
            inner: FileCatalogSource::with_provenance(
                path,
                watch_enabled,
                poll_interval,
                CONFIGMAP_PROVENANCE_SOURCE,
            ),
        }
    }

    pub fn from_mount(
        mount_dir: impl Into<PathBuf>,
        key: &str,
        watch_enabled: bool,
    ) -> anyhow::Result<Self> {
        validate_configmap_key(key)?;
        Ok(Self::new(mount_dir.into().join(key), watch_enabled))
    }

    pub fn path(&self) -> &Path {
        self.inner.path()
    }
}

impl CatalogSource for ConfigMapCatalogSource {
    fn load(&self) -> anyhow::Result<SourceSnapshot> {
        self.inner.load()
    }

    fn watch(&self) -> Option<mpsc::UnboundedReceiver<SourceSnapshot>> {
        self.inner.watch()
    }
}

/// Read-only environment-variable catalog source.
///
/// The variable may contain inline JSON, or `@/path/to/catalog.json` to keep large catalog
/// content out of process environment metadata. Environment sources do not support watch reloads.
#[derive(Debug, Clone)]
pub struct EnvCatalogSource {
    var: String,
}

impl EnvCatalogSource {
    pub fn new(var: impl Into<String>) -> Self {
        Self { var: var.into() }
    }

    pub fn var(&self) -> &str {
        &self.var
    }
}

impl CatalogSource for EnvCatalogSource {
    fn load(&self) -> anyhow::Result<SourceSnapshot> {
        let raw = std::env::var(&self.var)
            .with_context(|| format!("catalog environment variable {} is not set", self.var))?;
        if raw.trim().is_empty() {
            bail!(
                "catalog environment variable {} must not be empty",
                self.var
            );
        }

        let (data, uri) = if let Some(path) = raw.strip_prefix('@') {
            if path.is_empty() {
                bail!(
                    "catalog environment variable {} uses @path syntax with an empty path",
                    self.var
                );
            }
            let path = PathBuf::from(path);
            (
                fs::read(&path)
                    .with_context(|| format!("failed to read catalog file {}", path.display()))?,
                format!("env:{}@{}", self.var, path.display()),
            )
        } else {
            (raw.into_bytes(), format!("env:{}", self.var))
        };

        let raw_catalog: Value = serde_json::from_slice(&data).with_context(|| {
            format!(
                "catalog environment variable {} is not valid JSON",
                self.var
            )
        })?;
        if !raw_catalog.is_object() {
            bail!(
                "catalog environment variable {} must contain a JSON object",
                self.var
            );
        }
        let fingerprint = content_fingerprint(&data);
        let version = raw_catalog
            .get("version")
            .and_then(Value::as_str)
            .filter(|version| !version.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| fingerprint.clone());
        let provenance = raw_catalog
            .get("provenance")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_else(|| {
                let mut provenance = Map::new();
                provenance.insert(
                    "source".to_string(),
                    Value::String(ENV_PROVENANCE_SOURCE.to_string()),
                );
                provenance.insert("uri".to_string(), Value::String(uri));
                provenance.insert(
                    "contentHash".to_string(),
                    Value::String(fingerprint.clone()),
                );
                provenance
            });

        Ok(SourceSnapshot {
            raw_catalog,
            version,
            provenance,
            fingerprint,
        })
    }

    fn watch(&self) -> Option<mpsc::UnboundedReceiver<SourceSnapshot>> {
        None
    }
}

/// Read-only source useful for tests and future read-only backends.
#[derive(Debug, Clone)]
pub struct ReadOnlyCatalogSource {
    snapshot: SourceSnapshot,
}

impl ReadOnlyCatalogSource {
    pub fn new(snapshot: SourceSnapshot) -> Self {
        Self { snapshot }
    }
}

impl CatalogSource for ReadOnlyCatalogSource {
    fn load(&self) -> anyhow::Result<SourceSnapshot> {
        Ok(self.snapshot.clone())
    }

    fn watch(&self) -> Option<mpsc::UnboundedReceiver<SourceSnapshot>> {
        None
    }
}

pub fn source_from_descriptor(descriptor: &Value) -> anyhow::Result<Box<dyn CatalogSource>> {
    let object = descriptor.as_object().ok_or_else(|| {
        anyhow!("component.global.configComponent.catalogSource must be an object")
    })?;
    let source_type = object
        .get("type")
        .and_then(Value::as_str)
        .filter(|source_type| !source_type.is_empty())
        .ok_or_else(|| anyhow!("catalog source requires a non-empty type"))?;
    let watch = parse_watch_flag(object.get("watch"))?;

    match source_type {
        "file" => {
            let path = required_string(object, "path", "file catalog source")?;
            Ok(Box::new(FileCatalogSource::new(path, watch)))
        }
        "configmap" => {
            let source = if let Some(path) = optional_string(object, "path")? {
                ConfigMapCatalogSource::new(path, watch)
            } else {
                let mount_dir =
                    optional_string(object, "mountDir")?.unwrap_or(DEFAULT_CONFIGMAP_MOUNT_DIR);
                let key = optional_string(object, "key")?.unwrap_or(DEFAULT_CONFIGMAP_CATALOG_KEY);
                ConfigMapCatalogSource::from_mount(mount_dir, key, watch)?
            };
            Ok(Box::new(source))
        }
        "env" => {
            if watch {
                bail!("env catalog source does not support watch");
            }
            let var = optional_string(object, "var")?.unwrap_or(DEFAULT_ENV_CATALOG_VAR);
            Ok(Box::new(EnvCatalogSource::new(var)))
        }
        other => bail!(
            "unsupported catalogSource.type '{other}'; supported values are 'file', 'configmap', and 'env'"
        ),
    }
}

fn parse_watch_flag(value: Option<&Value>) -> anyhow::Result<bool> {
    value.map_or(Ok(false), |value| {
        value
            .as_bool()
            .ok_or_else(|| anyhow!("catalogSource.watch must be a boolean when present"))
    })
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    field: &str,
    context: &str,
) -> anyhow::Result<&'a str> {
    optional_string(object, field)?.ok_or_else(|| anyhow!("{context} requires a non-empty {field}"))
}

fn optional_string<'a>(
    object: &'a Map<String, Value>,
    field: &str,
) -> anyhow::Result<Option<&'a str>> {
    match object.get(field) {
        None => Ok(None),
        Some(Value::String(value)) if !value.is_empty() => Ok(Some(value.as_str())),
        Some(Value::String(_)) => Ok(None),
        Some(_) => Err(anyhow!(
            "catalogSource.{field} must be a string when present"
        )),
    }
}

fn validate_configmap_key(key: &str) -> anyhow::Result<()> {
    let path = Path::new(key);
    if key.is_empty() || path.components().count() != 1 {
        bail!("configmap catalog source key must be a file name, not a path");
    }
    Ok(())
}

fn content_fingerprint(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{digest:x}")
}
