//! Catalog parsing, validation, and protocol body helpers.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Map, Value};
use thiserror::Error;

use crate::tokens::{sanitize_token, short_component_token};

pub const BAD_REQUEST: &str = "BAD_REQUEST";
pub const CONFIG_NOT_FOUND: &str = "CONFIG_NOT_FOUND";
pub const CATALOG_UNAVAILABLE: &str = "CATALOG_UNAVAILABLE";
pub const CATALOG_INVALID: &str = "CATALOG_INVALID";
pub const CATALOG_UPDATE_DISABLED: &str = "CATALOG_UPDATE_DISABLED";
pub const LINEAGE_CYCLE: &str = "LINEAGE_CYCLE";
pub const LINEAGE_PARENT_MISSING: &str = "LINEAGE_PARENT_MISSING";
pub const LINEAGE_DEPTH_EXCEEDED: &str = "LINEAGE_DEPTH_EXCEEDED";
pub const LINEAGE_SCOPE_CONFLICT: &str = "LINEAGE_SCOPE_CONFLICT";
pub const LINEAGE_IDENTITY_CONFLICT: &str = "LINEAGE_IDENTITY_CONFLICT";

const LINEAGE_VERSION: i64 = 1;
const MAX_LINEAGE_DEPTH: usize = 64;
const DEVICE_LEVEL: &str = "device";

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
    pub override_provenance: bool,
    pub require_explicit_version: bool,
}

/// A hierarchy node from the ConfigComponent catalog.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogNode {
    pub parent: Option<String>,
    pub scope: Map<String, Value>,
    pub config: Value,
}

/// A component leaf from the ConfigComponent catalog.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogComponent {
    pub parent: Option<String>,
    pub config: Value,
}

/// Validated hierarchical catalog served by the ConfigComponent.
#[derive(Debug, Clone, PartialEq)]
pub struct Catalog {
    pub version: String,
    pub provenance: Map<String, Value>,
    pub hierarchy_levels: Vec<String>,
    pub nodes: HashMap<String, CatalogNode>,
    pub components: Vec<(String, CatalogComponent)>,
    pub raw: Value,
}

impl Catalog {
    /// Parse and validate a raw catalog object.
    pub fn parse(raw: Value, options: CatalogParseOptions) -> Result<Self, CatalogError> {
        let mut object = raw
            .as_object()
            .cloned()
            .ok_or_else(|| CatalogError::new(CATALOG_INVALID, "Catalog must be a JSON object"))?;

        if object.contains_key("base") {
            return Err(CatalogError::new(
                CATALOG_INVALID,
                "Old split catalogs with top-level base are not supported",
            ));
        }

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

        let provenance = if options.override_provenance {
            let provenance = options.source_provenance.unwrap_or_default();
            if !provenance.is_empty() {
                object.insert("provenance".to_string(), Value::Object(provenance.clone()));
            } else {
                object.remove("provenance");
            }
            provenance
        } else {
            match object.get("provenance") {
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
            }
        };

        let hierarchy_levels = parse_hierarchy_levels(object.get("hierarchy"))?;
        let level_set = hierarchy_levels
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();

        let nodes = parse_nodes(object.get("nodes"), &level_set)?;
        let components = parse_components(object.get("components"), &level_set)?;

        let catalog = Self {
            version,
            provenance,
            hierarchy_levels,
            nodes,
            components,
            raw: Value::Object(object),
        };
        catalog.validate_all_lineages()?;
        Ok(catalog)
    }

    /// Build the lineage bundle for a requested component token.
    pub fn lineage_for(&self, component: &str) -> Result<Value, CatalogError> {
        let token = short_component_token(component)
            .map_err(|message| CatalogError::new(BAD_REQUEST, message))?;
        self.lineage_for_token(&token)
    }

    /// Build lineage bundles for every component in the catalog.
    pub fn lineages(&self) -> Vec<(String, Value)> {
        self.components
            .iter()
            .filter_map(|(token, _)| {
                self.lineage_for_token(token)
                    .ok()
                    .map(|lineage| (token.clone(), lineage))
            })
            .collect()
    }

    fn lineage_for_token(&self, token: &str) -> Result<Value, CatalogError> {
        let component = self
            .components
            .iter()
            .find_map(|(candidate, component)| (candidate == token).then_some(component))
            .ok_or_else(|| {
                CatalogError::new(
                    CONFIG_NOT_FOUND,
                    format!("No configuration catalog entry for component '{token}'"),
                )
            })?;
        let layers = self.layers_for_component(token, component)?;

        let mut body = Map::new();
        body.insert(
            "lineageVersion".to_string(),
            Value::Number(LINEAGE_VERSION.into()),
        );
        body.insert(
            "catalogVersion".to_string(),
            Value::String(self.version.clone()),
        );
        body.insert("component".to_string(), Value::String(token.to_string()));
        if !self.provenance.is_empty() {
            body.insert(
                "provenance".to_string(),
                Value::Object(self.provenance.clone()),
            );
        }
        body.insert("layers".to_string(), Value::Array(layers));
        Ok(Value::Object(body))
    }

    fn layers_for_component(
        &self,
        token: &str,
        component: &CatalogComponent,
    ) -> Result<Vec<Value>, CatalogError> {
        let mut leaf_to_root = Vec::new();
        let mut seen = HashSet::new();
        let mut parent = component.parent.as_deref();

        while let Some(node_id) = parent {
            if leaf_to_root.len() >= MAX_LINEAGE_DEPTH {
                return Err(CatalogError::new(
                    LINEAGE_DEPTH_EXCEEDED,
                    format!(
                        "Catalog lineage for component '{token}' exceeds depth {MAX_LINEAGE_DEPTH}"
                    ),
                ));
            }
            if !seen.insert(node_id.to_string()) {
                return Err(CatalogError::new(
                    LINEAGE_CYCLE,
                    format!("Catalog lineage for component '{token}' revisits node '{node_id}'"),
                ));
            }
            let node = self.nodes.get(node_id).ok_or_else(|| {
                CatalogError::new(
                    LINEAGE_PARENT_MISSING,
                    format!("Catalog lineage for component '{token}' references missing parent '{node_id}'"),
                )
            })?;
            leaf_to_root.push(node_id.to_string());
            parent = node.parent.as_deref();
        }

        let mut cumulative_scope = HashMap::new();
        let mut cumulative_identity = HashMap::new();
        let mut layers = Vec::with_capacity(leaf_to_root.len() + 1);
        let hierarchy_levels = self
            .hierarchy_levels
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();

        for node_id in leaf_to_root.iter().rev() {
            let node = self.nodes.get(node_id).ok_or_else(|| {
                CatalogError::new(
                    LINEAGE_PARENT_MISSING,
                    format!("Catalog lineage for component '{token}' references missing parent '{node_id}'"),
                )
            })?;
            merge_scope(&mut cumulative_scope, &node.scope)?;
            merge_identity(
                &mut cumulative_identity,
                &cumulative_scope,
                &node.config,
                &hierarchy_levels,
            )?;
            layers.push(scope_layer(node_id, node));
        }

        merge_identity(
            &mut cumulative_identity,
            &cumulative_scope,
            &component.config,
            &hierarchy_levels,
        )?;
        layers.push(json!({
            "id": format!("component/{token}"),
            "kind": "component",
            "component": token,
            "config": component.config.clone()
        }));
        Ok(layers)
    }

    fn validate_all_lineages(&self) -> Result<(), CatalogError> {
        for node_id in self.nodes.keys() {
            self.validate_node_lineage(node_id)?;
        }
        for (token, component) in &self.components {
            self.layers_for_component(token, component)?;
        }
        Ok(())
    }

    fn validate_node_lineage(&self, start_id: &str) -> Result<(), CatalogError> {
        let mut leaf_to_root = Vec::new();
        let mut seen = HashSet::new();
        let mut current = Some(start_id);

        while let Some(node_id) = current {
            if leaf_to_root.len() >= MAX_LINEAGE_DEPTH {
                return Err(CatalogError::new(
                    LINEAGE_DEPTH_EXCEEDED,
                    format!(
                        "Catalog lineage for node '{start_id}' exceeds depth {MAX_LINEAGE_DEPTH}"
                    ),
                ));
            }
            if !seen.insert(node_id.to_string()) {
                return Err(CatalogError::new(
                    LINEAGE_CYCLE,
                    format!("Catalog lineage for node '{start_id}' revisits node '{node_id}'"),
                ));
            }
            let node = self.nodes.get(node_id).ok_or_else(|| {
                CatalogError::new(
                    LINEAGE_PARENT_MISSING,
                    format!(
                        "Catalog lineage for node '{start_id}' references missing parent '{node_id}'"
                    ),
                )
            })?;
            leaf_to_root.push(node_id.to_string());
            current = node.parent.as_deref();
        }

        let mut cumulative_scope = HashMap::new();
        let mut cumulative_identity = HashMap::new();
        let hierarchy_levels = self
            .hierarchy_levels
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();

        for node_id in leaf_to_root.iter().rev() {
            let node = self.nodes.get(node_id).ok_or_else(|| {
                CatalogError::new(
                    LINEAGE_PARENT_MISSING,
                    format!(
                        "Catalog lineage for node '{start_id}' references missing parent '{node_id}'"
                    ),
                )
            })?;
            merge_scope(&mut cumulative_scope, &node.scope)?;
            merge_identity(
                &mut cumulative_identity,
                &cumulative_scope,
                &node.config,
                &hierarchy_levels,
            )?;
        }
        Ok(())
    }
}

fn parse_hierarchy_levels(value: Option<&Value>) -> Result<Vec<String>, CatalogError> {
    let hierarchy = value
        .and_then(Value::as_object)
        .ok_or_else(|| CatalogError::new(CATALOG_INVALID, "Catalog hierarchy must be an object"))?;
    let levels = hierarchy
        .get("levels")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            CatalogError::new(CATALOG_INVALID, "Catalog hierarchy.levels must be an array")
        })?;
    if levels.is_empty() {
        return Err(CatalogError::new(
            CATALOG_INVALID,
            "Catalog hierarchy.levels must not be empty",
        ));
    }

    let mut seen = HashSet::new();
    let mut parsed = Vec::with_capacity(levels.len());
    for level in levels {
        let Some(level) = level.as_str().filter(|level| !level.is_empty()) else {
            return Err(CatalogError::new(
                CATALOG_INVALID,
                "Catalog hierarchy levels must be non-empty strings",
            ));
        };
        if !seen.insert(level.to_string()) {
            return Err(CatalogError::new(
                CATALOG_INVALID,
                format!("Catalog hierarchy level '{level}' is duplicated"),
            ));
        }
        parsed.push(level.to_string());
    }
    if parsed.last().map(String::as_str) != Some(DEVICE_LEVEL) {
        return Err(CatalogError::new(
            CATALOG_INVALID,
            "Catalog hierarchy.levels must end with device",
        ));
    }
    Ok(parsed)
}

fn parse_nodes(
    value: Option<&Value>,
    hierarchy_levels: &HashSet<&str>,
) -> Result<HashMap<String, CatalogNode>, CatalogError> {
    let object = value
        .and_then(Value::as_object)
        .ok_or_else(|| CatalogError::new(CATALOG_INVALID, "Catalog nodes must be an object"))?;
    let mut nodes = HashMap::new();
    for (id, entry) in object {
        validate_node_id(id, hierarchy_levels)?;
        let entry = entry.as_object().ok_or_else(|| {
            CatalogError::new(
                CATALOG_INVALID,
                format!("Catalog node '{id}' must be an object"),
            )
        })?;
        let parent = parse_parent(entry, &format!("Catalog node '{id}'"))?;
        let scope = parse_scope(entry.get("scope"), hierarchy_levels)?;
        validate_node_scope_matches_id(id, &scope)?;
        let config = parse_config(entry, &format!("Catalog node '{id}'"))?;
        identity_from_config(&config, hierarchy_levels)?;
        nodes.insert(
            id.clone(),
            CatalogNode {
                parent,
                scope,
                config,
            },
        );
    }
    Ok(nodes)
}

fn parse_components(
    value: Option<&Value>,
    hierarchy_levels: &HashSet<&str>,
) -> Result<Vec<(String, CatalogComponent)>, CatalogError> {
    let object = value.and_then(Value::as_object).ok_or_else(|| {
        CatalogError::new(CATALOG_INVALID, "Catalog components must be an object")
    })?;
    let mut components = Vec::with_capacity(object.len());
    for (key, entry) in object {
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
        let entry = entry.as_object().ok_or_else(|| {
            CatalogError::new(
                CATALOG_INVALID,
                format!("Catalog component entry '{key}' must be an object"),
            )
        })?;
        let parent = parse_parent(entry, &format!("Catalog component '{key}'"))?;
        let config = parse_config(entry, &format!("Catalog component '{key}'"))?;
        identity_from_config(&config, hierarchy_levels)?;
        components.push((key.clone(), CatalogComponent { parent, config }));
    }
    Ok(components)
}

fn validate_node_id(id: &str, hierarchy_levels: &HashSet<&str>) -> Result<(), CatalogError> {
    let Some((level, value)) = id.split_once('/') else {
        return Err(CatalogError::new(
            CATALOG_INVALID,
            format!("Catalog node id '{id}' must be '<level>/<value>'"),
        ));
    };
    if level.is_empty() || value.is_empty() {
        return Err(CatalogError::new(
            CATALOG_INVALID,
            format!("Catalog node id '{id}' must include non-empty level and value"),
        ));
    }
    if level == DEVICE_LEVEL {
        return Err(CatalogError::new(
            CATALOG_INVALID,
            "Catalog nodes must not describe device scope",
        ));
    }
    if !hierarchy_levels.contains(level) {
        return Err(CatalogError::new(
            CATALOG_INVALID,
            format!("Catalog node id '{id}' uses unknown hierarchy level '{level}'"),
        ));
    }
    Ok(())
}

fn parse_parent(entry: &Map<String, Value>, context: &str) -> Result<Option<String>, CatalogError> {
    match entry.get("parent") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(parent)) if !parent.is_empty() => Ok(Some(parent.clone())),
        Some(Value::String(_)) => Err(CatalogError::new(
            CATALOG_INVALID,
            format!("{context} parent must be non-empty when present"),
        )),
        Some(_) => Err(CatalogError::new(
            CATALOG_INVALID,
            format!("{context} parent must be a string when present"),
        )),
    }
}

fn parse_scope(
    value: Option<&Value>,
    hierarchy_levels: &HashSet<&str>,
) -> Result<Map<String, Value>, CatalogError> {
    let value = value
        .ok_or_else(|| CatalogError::new(CATALOG_INVALID, "Catalog node scope is required"))?;
    let scope = value.as_object().ok_or_else(|| {
        CatalogError::new(CATALOG_INVALID, "Catalog node scope must be an object")
    })?;
    if scope.is_empty() {
        return Err(CatalogError::new(
            CATALOG_INVALID,
            "Catalog node scope must not be empty",
        ));
    }
    validate_string_map(scope, hierarchy_levels, "scope")?;
    if scope.contains_key(DEVICE_LEVEL) {
        return Err(CatalogError::new(
            CATALOG_INVALID,
            "Catalog scope must not include device",
        ));
    }
    Ok(scope.clone())
}

fn validate_node_scope_matches_id(
    id: &str,
    scope: &Map<String, Value>,
) -> Result<(), CatalogError> {
    let Some((level, value)) = id.split_once('/') else {
        return Err(CatalogError::new(
            CATALOG_INVALID,
            format!("Catalog node id '{id}' must be '<level>/<value>'"),
        ));
    };
    match scope.get(level).and_then(Value::as_str) {
        Some(scope_value) if scope_value == value => Ok(()),
        Some(scope_value) => Err(CatalogError::new(
            CATALOG_INVALID,
            format!(
                "Catalog node '{id}' scope for '{level}' must be '{value}', not '{scope_value}'"
            ),
        )),
        None => Err(CatalogError::new(
            CATALOG_INVALID,
            format!("Catalog node '{id}' scope must include '{level}'"),
        )),
    }
}

fn parse_config(entry: &Map<String, Value>, context: &str) -> Result<Value, CatalogError> {
    match entry.get("config") {
        Some(Value::Object(config)) => Ok(Value::Object(config.clone())),
        Some(_) => Err(CatalogError::new(
            CATALOG_INVALID,
            format!("{context} config must be an object"),
        )),
        None => Err(CatalogError::new(
            CATALOG_INVALID,
            format!("{context} config is required"),
        )),
    }
}

fn validate_string_map(
    map: &Map<String, Value>,
    hierarchy_levels: &HashSet<&str>,
    context: &str,
) -> Result<(), CatalogError> {
    for (key, value) in map {
        if key.is_empty() {
            return Err(CatalogError::new(
                CATALOG_INVALID,
                format!("Catalog {context} keys must be non-empty strings"),
            ));
        }
        if !hierarchy_levels.contains(key.as_str()) {
            return Err(CatalogError::new(
                CATALOG_INVALID,
                format!("Catalog {context} key '{key}' is not in hierarchy.levels"),
            ));
        }
        if !value.as_str().is_some_and(|value| !value.is_empty()) {
            return Err(CatalogError::new(
                CATALOG_INVALID,
                format!("Catalog {context}.{key} must be a non-empty string"),
            ));
        }
    }
    Ok(())
}

fn identity_from_config(
    config: &Value,
    hierarchy_levels: &HashSet<&str>,
) -> Result<HashMap<String, String>, CatalogError> {
    let Some(identity) = config.get("identity") else {
        return Ok(HashMap::new());
    };
    let identity = identity.as_object().ok_or_else(|| {
        CatalogError::new(CATALOG_INVALID, "Catalog config identity must be an object")
    })?;
    validate_string_map(identity, hierarchy_levels, "identity")?;
    if identity.contains_key(DEVICE_LEVEL) {
        return Err(CatalogError::new(
            CATALOG_INVALID,
            "Catalog identity must not include device",
        ));
    }
    Ok(identity
        .iter()
        .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.to_string())))
        .collect())
}

fn merge_scope(
    cumulative: &mut HashMap<String, String>,
    scope: &Map<String, Value>,
) -> Result<(), CatalogError> {
    for (key, value) in scope {
        let value = value.as_str().ok_or_else(|| {
            CatalogError::new(
                CATALOG_INVALID,
                format!("Catalog scope.{key} must be a string"),
            )
        })?;
        if let Some(existing) = cumulative.get(key) {
            if existing != value {
                return Err(CatalogError::new(
                    LINEAGE_SCOPE_CONFLICT,
                    format!(
                        "Catalog lineage scope conflict for '{key}': '{existing}' != '{value}'"
                    ),
                ));
            }
        } else {
            cumulative.insert(key.clone(), value.to_string());
        }
    }
    Ok(())
}

fn merge_identity(
    cumulative_identity: &mut HashMap<String, String>,
    cumulative_scope: &HashMap<String, String>,
    config: &Value,
    hierarchy_levels: &HashSet<&str>,
) -> Result<(), CatalogError> {
    let identity = identity_from_config(config, hierarchy_levels)?;
    for (key, value) in identity {
        if let Some(scope_value) = cumulative_scope.get(&key) {
            if scope_value != &value {
                return Err(CatalogError::new(
                    LINEAGE_IDENTITY_CONFLICT,
                    format!(
                        "Catalog lineage identity conflict for '{key}': scope '{scope_value}' != identity '{value}'"
                    ),
                ));
            }
        }
        if let Some(existing) = cumulative_identity.get(&key) {
            if existing != &value {
                return Err(CatalogError::new(
                    LINEAGE_IDENTITY_CONFLICT,
                    format!(
                        "Catalog lineage identity conflict for '{key}': '{existing}' != '{value}'"
                    ),
                ));
            }
        } else {
            cumulative_identity.insert(key, value);
        }
    }
    Ok(())
}

fn scope_layer(id: &str, node: &CatalogNode) -> Value {
    let mut layer = Map::new();
    layer.insert("id".to_string(), Value::String(id.to_string()));
    layer.insert("kind".to_string(), Value::String("scope".to_string()));
    layer.insert("scope".to_string(), Value::Object(node.scope.clone()));
    layer.insert("config".to_string(), node.config.clone());
    Value::Object(layer)
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
