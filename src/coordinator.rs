//! Catalog promotion, update, reload, and push-bundle generation.

use std::sync::{Arc, RwLock};

use serde_json::Value;
use tokio::sync::mpsc;

use crate::catalog::{
    error_body, success_body, Catalog, CatalogError, CatalogParseOptions, BAD_REQUEST,
    CATALOG_INVALID, CATALOG_UNAVAILABLE, CATALOG_UPDATE_DISABLED,
};
use crate::source::{CatalogSource, SourceSnapshot};

/// A complete `set-config` push generated from an accepted catalog.
#[derive(Debug, Clone, PartialEq)]
pub struct PushBundle {
    pub topic: String,
    pub body: Value,
    pub version: String,
}

/// Result of a message-delivered catalog update.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateResult {
    pub ack: Value,
    pub pushes: Vec<PushBundle>,
}

#[derive(Debug, Clone, Default)]
struct CoordinatorState {
    active: Option<Catalog>,
    active_fingerprint: Option<String>,
}

/// Owns the active catalog snapshot and reject-and-keep update semantics.
pub struct CatalogCoordinator {
    source: Arc<dyn CatalogSource>,
    device_token: String,
    push_on_catalog_reload: bool,
    allow_volatile_catalog_updates: bool,
    state: RwLock<CoordinatorState>,
}

impl CatalogCoordinator {
    pub fn new(
        source: Arc<dyn CatalogSource>,
        device_token: impl Into<String>,
        push_on_catalog_reload: bool,
    ) -> Self {
        Self::with_volatile_updates(source, device_token, push_on_catalog_reload, false)
    }

    pub fn with_volatile_updates(
        source: Arc<dyn CatalogSource>,
        device_token: impl Into<String>,
        push_on_catalog_reload: bool,
        allow_volatile_catalog_updates: bool,
    ) -> Self {
        Self {
            source,
            device_token: device_token.into(),
            push_on_catalog_reload,
            allow_volatile_catalog_updates,
            state: RwLock::new(CoordinatorState::default()),
        }
    }

    pub fn active(&self) -> Option<Catalog> {
        self.state
            .read()
            .ok()
            .and_then(|state| state.active.clone())
    }

    pub fn watch_source(&self) -> Option<mpsc::UnboundedReceiver<SourceSnapshot>> {
        self.source.watch()
    }

    pub fn load_initial(&self) -> bool {
        match self.source.load().and_then(|snapshot| {
            self.promote_source_snapshot(snapshot)
                .map(|_| ())
                .map_err(Into::into)
        }) {
            Ok(()) => true,
            Err(error) => {
                tracing::error!(error = %error, "no valid configuration catalog is currently loaded");
                false
            }
        }
    }

    pub fn reload_from_source_snapshot(&self, snapshot: SourceSnapshot) -> Vec<PushBundle> {
        if self
            .state
            .read()
            .ok()
            .and_then(|state| state.active_fingerprint.clone())
            .as_deref()
            == Some(snapshot.fingerprint.as_str())
        {
            tracing::debug!("catalog reload ignored; fingerprint already active");
            return Vec::new();
        }

        let catalog = match parse_source_snapshot(&snapshot, false) {
            Ok(catalog) => catalog,
            Err(error) => {
                tracing::error!(error = %error, "rejected invalid catalog reload from source");
                return Vec::new();
            }
        };

        self.promote(catalog.clone(), Some(snapshot.fingerprint));
        tracing::info!(version = %catalog.version, "promoted catalog reload");
        if self.push_on_catalog_reload {
            self.pushes_for(Some(&catalog))
        } else {
            Vec::new()
        }
    }

    pub fn reload_from_source(&self) -> Vec<PushBundle> {
        match self.source.load() {
            Ok(snapshot) => self.reload_from_source_snapshot(snapshot),
            Err(error) => {
                tracing::error!(error = %error, "catalog reload failed");
                Vec::new()
            }
        }
    }

    pub fn bundle_for_request(&self, body: &Value) -> Value {
        let Some(object) = body.as_object() else {
            return error_body(BAD_REQUEST, "GetConfiguration body must be an object");
        };
        let Some(component) = object
            .get("component")
            .and_then(Value::as_str)
            .filter(|component| !component.is_empty())
        else {
            return error_body(
                BAD_REQUEST,
                "GetConfiguration body must include string field 'component'",
            );
        };

        let Some(catalog) = self.active() else {
            return error_body(
                CATALOG_UNAVAILABLE,
                "No valid configuration catalog is loaded",
            );
        };

        match catalog.bundle_for(component) {
            Ok(bundle) => bundle,
            Err(error) => error.body(),
        }
    }

    pub fn update_from_message(&self, body: &Value) -> UpdateResult {
        let Some(object) = body.as_object() else {
            return UpdateResult {
                ack: error_body(BAD_REQUEST, "UpdateCatalog body must be an object"),
                pushes: Vec::new(),
            };
        };
        let Some(version) = object
            .get("version")
            .and_then(Value::as_str)
            .filter(|version| !version.is_empty())
        else {
            return UpdateResult {
                ack: error_body(
                    BAD_REQUEST,
                    "UpdateCatalog body must include string field 'version'",
                ),
                pushes: Vec::new(),
            };
        };
        let Some(raw_catalog) = object.get("catalog").filter(|value| value.is_object()) else {
            return UpdateResult {
                ack: error_body(
                    BAD_REQUEST,
                    "UpdateCatalog body must include object field 'catalog'",
                ),
                pushes: Vec::new(),
            };
        };
        if raw_catalog.get("version").and_then(Value::as_str) != Some(version) {
            return UpdateResult {
                ack: error_body(CATALOG_INVALID, "Catalog version must match update version"),
                pushes: Vec::new(),
            };
        }

        if !self.allow_volatile_catalog_updates {
            return UpdateResult {
                ack: error_body(
                    CATALOG_UPDATE_DISABLED,
                    "Volatile catalog updates are disabled",
                ),
                pushes: Vec::new(),
            };
        }

        let catalog = match Catalog::parse(
            raw_catalog.clone(),
            CatalogParseOptions {
                source_provenance: Some(volatile_update_provenance()),
                require_explicit_version: true,
                ..CatalogParseOptions::default()
            },
        ) {
            Ok(catalog) => catalog,
            Err(error) => {
                return UpdateResult {
                    ack: error.body(),
                    pushes: Vec::new(),
                };
            }
        };

        self.promote(
            catalog.clone(),
            Some(format!("volatile-message-update:{version}")),
        );
        let pushes = if self.push_on_catalog_reload {
            self.pushes_for(Some(&catalog))
        } else {
            Vec::new()
        };
        UpdateResult {
            ack: success_body(&catalog.version, &catalog.provenance),
            pushes,
        }
    }

    pub fn pushes_for(&self, catalog: Option<&Catalog>) -> Vec<PushBundle> {
        let owned;
        let catalog = match catalog {
            Some(catalog) => catalog,
            None => {
                owned = self.active();
                match owned.as_ref() {
                    Some(catalog) => catalog,
                    None => return Vec::new(),
                }
            }
        };

        catalog
            .bundles()
            .into_iter()
            .map(|(token, body)| PushBundle {
                topic: format!("ecv1/{}/{token}/main/cmd/set-config", self.device_token),
                body,
                version: catalog.version.clone(),
            })
            .collect()
    }

    fn promote_source_snapshot(&self, snapshot: SourceSnapshot) -> Result<Catalog, CatalogError> {
        let catalog = parse_source_snapshot(&snapshot, false)?;
        self.promote(catalog.clone(), Some(snapshot.fingerprint));
        Ok(catalog)
    }

    fn promote(&self, catalog: Catalog, fingerprint: Option<String>) {
        if let Ok(mut state) = self.state.write() {
            state.active = Some(catalog);
            state.active_fingerprint = fingerprint;
        }
    }
}

fn volatile_update_provenance() -> serde_json::Map<String, Value> {
    serde_json::Map::from_iter([
        ("source".to_string(), Value::String("message".to_string())),
        (
            "interface".to_string(),
            Value::String("update-catalog".to_string()),
        ),
        ("volatile".to_string(), Value::Bool(true)),
    ])
}

fn parse_source_snapshot(
    snapshot: &SourceSnapshot,
    require_explicit_version: bool,
) -> Result<Catalog, CatalogError> {
    Catalog::parse(
        snapshot.raw_catalog.clone(),
        CatalogParseOptions {
            derived_version: Some(snapshot.version.clone()),
            source_provenance: Some(snapshot.provenance.clone()),
            require_explicit_version,
        },
    )
}
