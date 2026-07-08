//! Catalog parsing, validation, and protocol body helpers.

use serde_json::{json, Map, Value};
use thiserror::Error;

use crate::tokens::{sanitize_token, short_component_token};

pub const BAD_REQUEST: &str = "BAD_REQUEST";
pub const CONFIG_NOT_FOUND: &str = "CONFIG_NOT_FOUND";
pub const CATALOG_UNAVAILABLE: &str = "CATALOG_UNAVAILABLE";
pub const CATALOG_INVALID: &str = "CATALOG_INVALID";
pub const CATALOG_UPDATE_DISABLED: &str = "CATALOG_UPDATE_DISABLED";

/// A structured ConfigComponent protocol error.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[error("{code}: {message}")]
pub struct CatalogError {
    pub code: &'static str,
    pub message: String,
}

impl CatalogError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn body(&self) -> Value {
        error_body(self.code, &self.message)
    }
}

/// Options for parsing a raw catalog snapshot.
#[derive(Debug, Clone, Default)]
pub struct CatalogParseOptions {
    pub derived_version: Option<String>,
    pub source_provenance: Option<Map<String, Value>>,
    pub require_explicit_version: bool,
}

/// Validated raw-layer catalog served by the ConfigComponent.
#[derive(Debug, Clone, PartialEq)]
pub struct Catalog {
    pub version: String,
    pub provenance: Map<String, Value>,
    pub base: Option<Value>,
    pub components: Vec<(String, Value)>,
    pub raw: Value,
}

impl Catalog {
    /// Parse and validate a raw catalog object.
    pub fn parse(raw: Value, options: CatalogParseOptions) -> Result<Self, CatalogError> {
        let mut object = raw
            .as_object()
            .cloned()
            .ok_or_else(|| CatalogError::new(CATALOG_INVALID, "Catalog must be a JSON object"))?;

        if object.get("schemaVersion").and_then(Value::as_i64) != Some(1) {
            return Err(CatalogError::new(
                CATALOG_INVALID,
                "Catalog schemaVersion must be 1",
            ));
        }

        let version = match object.get("version").and_then(Value::as_str) {
            Some(version) if !version.is_empty() => version.to_string(),
            _ if options.require_explicit_version => {
                return Err(CatalogError::new(
                    CATALOG_INVALID,
                    "Message-delivered catalogs must include version",
                ));
            }
            _ => match options.derived_version {
                Some(version) if !version.is_empty() => {
                    object.insert("version".to_string(), Value::String(version.clone()));
                    version
                }
                _ => {
                    return Err(CatalogError::new(
                        CATALOG_INVALID,
                        "Catalog version is required",
                    ));
                }
            },
        };

        let provenance = match object.get("provenance") {
            Some(Value::Object(provenance)) => provenance.clone(),
            Some(_) => {
                return Err(CatalogError::new(
                    CATALOG_INVALID,
                    "Catalog provenance must be an object",
                ));
            }
            None => {
                let provenance = options.source_provenance.unwrap_or_default();
                if !provenance.is_empty() {
                    object.insert("provenance".to_string(), Value::Object(provenance.clone()));
                }
                provenance
            }
        };

        let base = match object.get("base") {
            None | Some(Value::Null) => None,
            Some(Value::Object(base)) => {
                if base.contains_key("extends") {
                    return Err(CatalogError::new(
                        CATALOG_INVALID,
                        "Catalog base must not contain extends; N-layer inheritance is not implemented",
                    ));
                }
                Some(Value::Object(base.clone()))
            }
            Some(_) => {
                return Err(CatalogError::new(
                    CATALOG_INVALID,
                    "Catalog base must be an object or null",
                ));
            }
        };

        let components_object = object
            .get("components")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                CatalogError::new(CATALOG_INVALID, "Catalog components must be an object")
            })?;
        let mut components = Vec::with_capacity(components_object.len());
        for (key, value) in components_object {
            if key.is_empty() {
                return Err(CatalogError::new(
                    CATALOG_INVALID,
                    "Catalog component keys must be non-empty strings",
                ));
            }
            if sanitize_token(key) != *key {
                return Err(CatalogError::new(
                    CATALOG_INVALID,
                    format!("Catalog component key '{key}' is not a sanitized token"),
                ));
            }
            if !value.is_object() {
                return Err(CatalogError::new(
                    CATALOG_INVALID,
                    format!("Catalog component entry '{key}' must be an object"),
                ));
            }
            components.push((key.clone(), value.clone()));
        }

        Ok(Self {
            version,
            provenance,
            base,
            components,
            raw: Value::Object(object),
        })
    }

    /// Build the raw layer bundle for a requested component token.
    pub fn bundle_for(&self, component: &str) -> Result<Value, CatalogError> {
        let token = short_component_token(component)
            .map_err(|message| CatalogError::new(BAD_REQUEST, message))?;
        let layer = self
            .components
            .iter()
            .find_map(|(candidate, layer)| (candidate == &token).then_some(layer))
            .ok_or_else(|| {
                CatalogError::new(
                    CONFIG_NOT_FOUND,
                    format!("No configuration catalog entry for component '{token}'"),
                )
            })?;
        Ok(json!({
            "base": self.base.clone().unwrap_or(Value::Null),
            "component": layer.clone()
        }))
    }

    /// Build raw layer bundles for every component in the catalog.
    pub fn bundles(&self) -> Vec<(String, Value)> {
        self.components
            .iter()
            .map(|(token, layer)| {
                (
                    token.clone(),
                    json!({
                        "base": self.base.clone().unwrap_or(Value::Null),
                        "component": layer.clone()
                    }),
                )
            })
            .collect()
    }
}

pub fn error_body(code: &'static str, message: &str) -> Value {
    json!({
        "ok": false,
        "error": {
            "code": code,
            "message": message
        }
    })
}

pub fn success_body(version: &str, provenance: &Map<String, Value>) -> Value {
    let mut body = Map::new();
    body.insert("ok".to_string(), Value::Bool(true));
    body.insert("version".to_string(), Value::String(version.to_string()));
    if !provenance.is_empty() {
        body.insert("provenance".to_string(), Value::Object(provenance.clone()));
    }
    Value::Object(body)
}

pub fn is_error_body(value: &Value) -> bool {
    value.as_object().is_some_and(|body| {
        body.get("ok") == Some(&Value::Bool(false)) && body.get("error").is_some()
    })
}
