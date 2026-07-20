//! Focused unit coverage for the pure catalog / source / coordinator logic.
//!
//! The messaging runtime seam (`server.rs`, `main.rs`) needs a live `EdgeCommons` runtime + broker
//! and is validated by the scaffold→build gate and HOST/Greengrass smoke, not unit tests; it is the
//! only code excluded from the coverage gate. Everything reachable without a broker — catalog
//! parsing/lineage, the three catalog sources, the descriptor factory, and the coordinator's
//! promote/serve/update/reload logic — is exercised here so it stays in the 90% denominator.

use std::sync::{Arc, Mutex};

use config_component::catalog::{
    error_body, is_error_body, success_body, Catalog, CatalogError, CatalogParseOptions,
    BAD_REQUEST, CATALOG_INVALID,
};
use config_component::coordinator::CatalogCoordinator;
use config_component::source::{
    source_from_descriptor, CatalogSource, ConfigMapCatalogSource, EnvCatalogSource,
    FileCatalogSource, ReadOnlyCatalogSource, SourceSnapshot,
};
use serde_json::{json, Map, Value};
use tempfile::tempdir;

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

/// A minimal valid catalog: one scope node and one component parented to it.
fn valid_catalog() -> Value {
    json!({
        "schemaVersion": 1,
        "version": "v1",
        "hierarchy": { "levels": ["site", "device"] },
        "nodes": {
            "site/dallas": {
                "scope": { "site": "dallas" },
                "config": { "identity": { "site": "dallas" } }
            }
        },
        "components": {
            "opcua-adapter": {
                "parent": "site/dallas",
                "config": { "component": { "token": "opcua-adapter" } }
            }
        }
    })
}

fn parse(raw: Value) -> Result<Catalog, CatalogError> {
    Catalog::parse(raw, CatalogParseOptions::default())
}

/// Parse a catalog derived from `valid_catalog()` after applying `mutate`, expecting `code`.
fn expect_code(mutate: impl FnOnce(&mut Value), code: &str) {
    let mut raw = valid_catalog();
    mutate(&mut raw);
    let err = parse(raw).unwrap_err();
    assert_eq!(err.code, code, "expected {code}, got {}", err.code);
}

// ---------------------------------------------------------------------------------------------
// catalog.rs — happy path + protocol helpers
// ---------------------------------------------------------------------------------------------

#[test]
fn parses_and_serves_minimal_catalog() {
    let catalog = parse(valid_catalog()).unwrap();
    assert_eq!(catalog.version, "v1");
    assert_eq!(catalog.hierarchy_levels, vec!["site", "device"]);

    let bundle = catalog.lineage_for("opcua-adapter").unwrap();
    assert_eq!(bundle["component"], "opcua-adapter");
    let layers = bundle["layers"].as_array().unwrap();
    assert_eq!(layers[0]["id"], "site/dallas");
    assert_eq!(layers[1]["id"], "component/opcua-adapter");

    // Every component lineage is buildable.
    let all = catalog.lineages();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].0, "opcua-adapter");
}

#[test]
fn lineage_for_reduces_dotted_component_name_to_short_token() {
    // The component is keyed by its short token; a dotted GG name resolves to the same entry.
    let catalog = parse(valid_catalog()).unwrap();
    let bundle = catalog
        .lineage_for("com.mbreissi.edgecommons.opcua-adapter")
        .unwrap();
    assert_eq!(bundle["component"], "opcua-adapter");
}

#[test]
fn lineage_for_rejects_empty_component() {
    let catalog = parse(valid_catalog()).unwrap();
    let err = catalog.lineage_for("   ").unwrap_err();
    assert_eq!(err.code, BAD_REQUEST);
}

#[test]
fn provenance_is_carried_into_the_bundle() {
    let mut raw = valid_catalog();
    raw["provenance"] = json!({ "source": "file", "uri": "/etc/edgecommons/catalog.json" });
    let catalog = parse(raw).unwrap();
    let bundle = catalog.lineage_for("opcua-adapter").unwrap();
    assert_eq!(bundle["provenance"]["source"], "file");
}

#[test]
fn error_and_success_body_helpers_round_trip() {
    let err = error_body(BAD_REQUEST, "nope");
    assert_eq!(err["ok"], false);
    assert_eq!(err["error"]["code"], "BAD_REQUEST");
    assert!(is_error_body(&err));

    let mut provenance = Map::new();
    provenance.insert("source".into(), Value::String("message".into()));
    let ok = success_body("v9", &provenance);
    assert_eq!(ok["ok"], true);
    assert_eq!(ok["version"], "v9");
    assert_eq!(ok["provenance"]["source"], "message");
    assert!(!is_error_body(&ok));

    // Success without provenance omits the field.
    let ok_bare = success_body("v9", &Map::new());
    assert!(ok_bare.get("provenance").is_none());

    // A plain object is not an error body.
    assert!(!is_error_body(&json!({ "hello": "world" })));

    let structured = CatalogError::new(CATALOG_INVALID, "bad");
    assert_eq!(structured.body()["error"]["code"], "CATALOG_INVALID");
    assert_eq!(structured.to_string(), "CATALOG_INVALID: bad");
}

// ---------------------------------------------------------------------------------------------
// catalog.rs — parse/validation error branches
// ---------------------------------------------------------------------------------------------

#[test]
fn rejects_non_object_catalog() {
    assert_eq!(parse(json!("not an object")).unwrap_err().code, CATALOG_INVALID);
}

#[test]
fn rejects_bad_top_level_shape() {
    expect_code(|raw| raw["base"] = json!({}), CATALOG_INVALID);
    expect_code(|raw| raw["schemaVersion"] = json!(2), CATALOG_INVALID);
    expect_code(|raw| raw["provenance"] = json!("not-object"), CATALOG_INVALID);
}

#[test]
fn version_handling() {
    // Absent version with no derived version is rejected.
    expect_code(
        |raw| {
            raw.as_object_mut().unwrap().remove("version");
        },
        CATALOG_INVALID,
    );

    // A derived version is adopted when the catalog omits one.
    let mut raw = valid_catalog();
    raw.as_object_mut().unwrap().remove("version");
    let catalog = Catalog::parse(
        raw,
        CatalogParseOptions {
            derived_version: Some("derived-1".into()),
            ..CatalogParseOptions::default()
        },
    )
    .unwrap();
    assert_eq!(catalog.version, "derived-1");

    // require_explicit_version rejects a message catalog without one.
    let mut raw = valid_catalog();
    raw.as_object_mut().unwrap().remove("version");
    let err = Catalog::parse(
        raw,
        CatalogParseOptions {
            require_explicit_version: true,
            ..CatalogParseOptions::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, CATALOG_INVALID);
}

#[test]
fn provenance_override_modes() {
    // override_provenance with a non-empty source provenance replaces it.
    let mut src = Map::new();
    src.insert("source".into(), Value::String("message".into()));
    let catalog = Catalog::parse(
        valid_catalog(),
        CatalogParseOptions {
            source_provenance: Some(src),
            override_provenance: true,
            ..CatalogParseOptions::default()
        },
    )
    .unwrap();
    assert_eq!(catalog.provenance["source"], "message");

    // override_provenance with empty provenance strips any inline provenance.
    let mut raw = valid_catalog();
    raw["provenance"] = json!({ "source": "file" });
    let catalog = Catalog::parse(
        raw,
        CatalogParseOptions {
            source_provenance: Some(Map::new()),
            override_provenance: true,
            ..CatalogParseOptions::default()
        },
    )
    .unwrap();
    assert!(catalog.provenance.is_empty());

    // Non-override with no inline provenance falls back to the source provenance.
    let mut raw = valid_catalog();
    raw.as_object_mut().unwrap().remove("provenance");
    let mut src = Map::new();
    src.insert("source".into(), Value::String("file".into()));
    let catalog = Catalog::parse(
        raw,
        CatalogParseOptions {
            source_provenance: Some(src),
            ..CatalogParseOptions::default()
        },
    )
    .unwrap();
    assert_eq!(catalog.provenance["source"], "file");
}

#[test]
fn rejects_bad_hierarchy() {
    expect_code(|raw| raw["hierarchy"] = json!("x"), CATALOG_INVALID);
    expect_code(|raw| raw["hierarchy"]["levels"] = json!("x"), CATALOG_INVALID);
    expect_code(|raw| raw["hierarchy"]["levels"] = json!([]), CATALOG_INVALID);
    expect_code(|raw| raw["hierarchy"]["levels"] = json!(["site", 7, "device"]), CATALOG_INVALID);
    expect_code(
        |raw| raw["hierarchy"]["levels"] = json!(["site", "site", "device"]),
        CATALOG_INVALID,
    );
    // Must end with device.
    expect_code(|raw| raw["hierarchy"]["levels"] = json!(["site", "line"]), CATALOG_INVALID);
}

#[test]
fn rejects_bad_nodes() {
    expect_code(|raw| raw["nodes"] = json!("x"), CATALOG_INVALID);
    // Node id not <level>/<value>.
    expect_code(|raw| raw["nodes"] = json!({ "sitedallas": { "scope": {}, "config": {} } }), CATALOG_INVALID);
    // Empty level or value in id.
    expect_code(|raw| raw["nodes"] = json!({ "/dallas": { "scope": {}, "config": {} } }), CATALOG_INVALID);
    // Node describing device scope by id.
    expect_code(
        |raw| raw["nodes"]["device/edge-1"] = json!({ "scope": { "site": "dallas" }, "config": {} }),
        CATALOG_INVALID,
    );
    // Unknown hierarchy level in id.
    expect_code(
        |raw| raw["nodes"]["floor/2"] = json!({ "scope": { "floor": "2" }, "config": {} }),
        CATALOG_INVALID,
    );
    // Node entry not an object.
    expect_code(|raw| raw["nodes"]["site/x"] = json!("nope"), CATALOG_INVALID);
    // Parent non-string / empty.
    expect_code(|raw| raw["nodes"]["site/dallas"]["parent"] = json!(7), CATALOG_INVALID);
    expect_code(|raw| raw["nodes"]["site/dallas"]["parent"] = json!(""), CATALOG_INVALID);
}

#[test]
fn rejects_bad_node_scope() {
    // Missing scope.
    expect_code(
        |raw| {
            raw["nodes"]["site/dallas"]
                .as_object_mut()
                .unwrap()
                .remove("scope");
        },
        CATALOG_INVALID,
    );
    expect_code(|raw| raw["nodes"]["site/dallas"]["scope"] = json!("x"), CATALOG_INVALID);
    expect_code(|raw| raw["nodes"]["site/dallas"]["scope"] = json!({}), CATALOG_INVALID);
    // Scope key not in hierarchy.levels.
    expect_code(
        |raw| raw["nodes"]["site/dallas"]["scope"] = json!({ "floor": "2", "site": "dallas" }),
        CATALOG_INVALID,
    );
    // Scope includes device.
    expect_code(
        |raw| raw["nodes"]["site/dallas"]["scope"] = json!({ "site": "dallas", "device": "x" }),
        CATALOG_INVALID,
    );
    // Scope value not a non-empty string.
    expect_code(|raw| raw["nodes"]["site/dallas"]["scope"] = json!({ "site": "" }), CATALOG_INVALID);
    // Scope does not own the node id's own claim.
    expect_code(
        |raw| raw["nodes"]["site/dallas"]["scope"] = json!({ "site": "houston" }),
        CATALOG_INVALID,
    );
}

#[test]
fn rejects_bad_node_config_and_identity() {
    expect_code(
        |raw| {
            raw["nodes"]["site/dallas"]
                .as_object_mut()
                .unwrap()
                .remove("config");
        },
        CATALOG_INVALID,
    );
    expect_code(|raw| raw["nodes"]["site/dallas"]["config"] = json!("x"), CATALOG_INVALID);
    expect_code(|raw| raw["nodes"]["site/dallas"]["config"]["identity"] = json!("x"), CATALOG_INVALID);
    // Identity includes device.
    expect_code(
        |raw| raw["nodes"]["site/dallas"]["config"]["identity"] = json!({ "device": "edge-1" }),
        CATALOG_INVALID,
    );
    // Identity key not in hierarchy.
    expect_code(
        |raw| raw["nodes"]["site/dallas"]["config"]["identity"] = json!({ "floor": "2" }),
        CATALOG_INVALID,
    );
}

#[test]
fn rejects_bad_components() {
    expect_code(|raw| raw["components"] = json!("x"), CATALOG_INVALID);
    expect_code(|raw| raw["components"][""] = json!({ "config": {} }), CATALOG_INVALID);
    // Key not a sanitized token (contains a UNS-reserved character).
    expect_code(|raw| raw["components"]["a/b"] = json!({ "config": {} }), CATALOG_INVALID);
    // Entry not object.
    expect_code(|raw| raw["components"]["x"] = json!("nope"), CATALOG_INVALID);
    // Missing config.
    expect_code(|raw| raw["components"]["x"] = json!({}), CATALOG_INVALID);
    // Config not object.
    expect_code(|raw| raw["components"]["x"] = json!({ "config": "nope" }), CATALOG_INVALID);
    // Component parent non-string.
    expect_code(
        |raw| raw["components"]["opcua-adapter"]["parent"] = json!(7),
        CATALOG_INVALID,
    );
}

#[test]
fn rejects_broken_lineage() {
    // Component references a missing parent node.
    expect_code(
        |raw| raw["components"]["opcua-adapter"]["parent"] = json!("site/nowhere"),
        "LINEAGE_PARENT_MISSING",
    );
}

// ---------------------------------------------------------------------------------------------
// source.rs — FileCatalogSource
// ---------------------------------------------------------------------------------------------

#[test]
fn file_source_load_errors() {
    let source = FileCatalogSource::new("/no/such/catalog.json", false);
    assert!(source.load().is_err());
    assert!(source.watch().is_none());
    assert_eq!(source.path(), std::path::Path::new("/no/such/catalog.json"));

    let dir = tempdir().unwrap();
    let bad = dir.path().join("bad.json");
    std::fs::write(&bad, b"{ not json").unwrap();
    assert!(FileCatalogSource::new(&bad, false).load().is_err());

    let non_object = dir.path().join("arr.json");
    std::fs::write(&non_object, b"[1,2,3]").unwrap();
    assert!(FileCatalogSource::new(&non_object, false).load().is_err());
}

#[test]
fn file_source_uses_explicit_version_and_provenance() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("catalog.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({
            "version": "explicit-9",
            "provenance": { "source": "file", "uri": "custom" },
            "schemaVersion": 1
        }))
        .unwrap(),
    )
    .unwrap();
    let snapshot = FileCatalogSource::new(&path, false).load().unwrap();
    assert_eq!(snapshot.version, "explicit-9");
    assert_eq!(snapshot.provenance["uri"], "custom");
}

// ---------------------------------------------------------------------------------------------
// source.rs — EnvCatalogSource
// ---------------------------------------------------------------------------------------------

#[test]
fn env_source_reads_inline_json_and_derives_metadata() {
    let var = "EC_TEST_CATALOG_INLINE";
    std::env::set_var(var, r#"{"schemaVersion":1,"hierarchy":{"levels":["device"]}}"#);
    let source = EnvCatalogSource::new(var);
    assert_eq!(source.var(), var);
    let snapshot = source.load().unwrap();
    assert!(snapshot.version.starts_with("sha256:"));
    assert_eq!(snapshot.provenance["source"], "env");
    assert!(source.watch().is_none());
    std::env::remove_var(var);
}

#[test]
fn env_source_reads_at_path_reference() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("catalog.json");
    std::fs::write(&path, br#"{"schemaVersion":1,"version":"envfile-1"}"#).unwrap();
    let var = "EC_TEST_CATALOG_ATPATH";
    std::env::set_var(var, format!("@{}", path.display()));
    let snapshot = EnvCatalogSource::new(var).load().unwrap();
    assert_eq!(snapshot.version, "envfile-1");
    std::env::remove_var(var);
}

#[test]
fn env_source_error_paths() {
    let var = "EC_TEST_CATALOG_ERRORS";
    std::env::remove_var(var);
    assert!(EnvCatalogSource::new(var).load().is_err()); // unset

    std::env::set_var(var, "   ");
    assert!(EnvCatalogSource::new(var).load().is_err()); // empty

    std::env::set_var(var, "@");
    assert!(EnvCatalogSource::new(var).load().is_err()); // empty @path

    std::env::set_var(var, "@/no/such/file.json");
    assert!(EnvCatalogSource::new(var).load().is_err()); // missing @path file

    std::env::set_var(var, "not json");
    assert!(EnvCatalogSource::new(var).load().is_err()); // invalid JSON

    std::env::set_var(var, "[1,2,3]");
    assert!(EnvCatalogSource::new(var).load().is_err()); // not an object
    std::env::remove_var(var);
}

#[test]
fn read_only_source_returns_snapshot() {
    let snapshot = SourceSnapshot {
        raw_catalog: json!({ "schemaVersion": 1 }),
        version: "ro-1".into(),
        provenance: Map::new(),
        fingerprint: "fp".into(),
    };
    let source = ReadOnlyCatalogSource::new(snapshot.clone());
    assert_eq!(source.load().unwrap(), snapshot);
    assert!(source.watch().is_none());
}

#[test]
fn configmap_from_mount_rejects_pathy_key() {
    assert!(ConfigMapCatalogSource::from_mount("/etc/edgecommons", "sub/catalog.json", false).is_err());
    let ok = ConfigMapCatalogSource::from_mount("/etc/edgecommons", "catalog.json", false).unwrap();
    assert_eq!(ok.path(), std::path::Path::new("/etc/edgecommons/catalog.json"));
}

// ---------------------------------------------------------------------------------------------
// source.rs — source_from_descriptor
// ---------------------------------------------------------------------------------------------

#[test]
fn descriptor_factory_builds_every_source_type() {
    // file
    source_from_descriptor(&json!({ "type": "file", "path": "/x/catalog.json" })).unwrap();
    // configmap by path
    source_from_descriptor(&json!({ "type": "configmap", "path": "/etc/edgecommons/catalog.json" })).unwrap();
    // configmap by mountDir + key (defaults + explicit)
    source_from_descriptor(&json!({ "type": "configmap" })).unwrap();
    source_from_descriptor(&json!({ "type": "configmap", "mountDir": "/m", "key": "c.json" })).unwrap();
    // env with default var + explicit var
    source_from_descriptor(&json!({ "type": "env" })).unwrap();
    source_from_descriptor(&json!({ "type": "env", "var": "MY_CATALOG" })).unwrap();
}

#[test]
fn descriptor_factory_error_paths() {
    assert!(source_from_descriptor(&json!("x")).is_err()); // not an object
    assert!(source_from_descriptor(&json!({})).is_err()); // missing type
    assert!(source_from_descriptor(&json!({ "type": "" })).is_err()); // empty type
    assert!(source_from_descriptor(&json!({ "type": "file", "watch": 7 })).is_err()); // watch not bool
    assert!(source_from_descriptor(&json!({ "type": "file" })).is_err()); // file needs path
    assert!(source_from_descriptor(&json!({ "type": "env", "watch": true })).is_err()); // env can't watch
    assert!(source_from_descriptor(&json!({ "type": "configmap", "key": "a/b.json" })).is_err()); // pathy key
    assert!(source_from_descriptor(&json!({ "type": "bogus" })).is_err()); // unknown type
}

// ---------------------------------------------------------------------------------------------
// coordinator.rs — serve / update / reload branches
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
struct MemorySource {
    raw: Mutex<Option<Value>>,
}

impl MemorySource {
    fn new(raw: Option<Value>) -> Self {
        Self {
            raw: Mutex::new(raw),
        }
    }
}

impl CatalogSource for MemorySource {
    fn load(&self) -> anyhow::Result<SourceSnapshot> {
        let raw = self
            .raw
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no catalog"))?;
        let version = raw
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or("mem")
            .to_string();
        Ok(SourceSnapshot {
            raw_catalog: raw,
            version,
            provenance: Map::new(),
            fingerprint: "fp".into(),
        })
    }

    fn watch(&self) -> Option<tokio::sync::mpsc::UnboundedReceiver<SourceSnapshot>> {
        None
    }
}

fn loaded_coordinator(volatile: bool, push_on_reload: bool) -> CatalogCoordinator {
    let coordinator = CatalogCoordinator::with_volatile_updates(
        Arc::new(MemorySource::new(Some(valid_catalog()))),
        "edge-01",
        push_on_reload,
        volatile,
    );
    assert!(coordinator.load_initial());
    coordinator
}

#[test]
fn load_initial_reports_failure_on_invalid_source() {
    let coordinator =
        CatalogCoordinator::new(Arc::new(MemorySource::new(Some(json!({ "bogus": true })))), "edge-01", true);
    assert!(!coordinator.load_initial());
    assert!(coordinator.active().is_none());
}

#[test]
fn bundle_for_request_branches() {
    let coordinator = loaded_coordinator(false, true);

    // Non-object body.
    assert_eq!(
        coordinator.bundle_for_request(&json!("x"))["error"]["code"],
        "BAD_REQUEST"
    );
    // Missing/empty component field.
    assert_eq!(
        coordinator.bundle_for_request(&json!({}))["error"]["code"],
        "BAD_REQUEST"
    );
    assert_eq!(
        coordinator.bundle_for_request(&json!({ "component": "" }))["error"]["code"],
        "BAD_REQUEST"
    );
    // Unknown component.
    assert_eq!(
        coordinator.bundle_for_request(&json!({ "component": "nope" }))["error"]["code"],
        "CONFIG_NOT_FOUND"
    );
    // Valid request.
    let bundle = coordinator.bundle_for_request(&json!({ "component": "opcua-adapter" }));
    assert_eq!(bundle["component"], "opcua-adapter");

    // No catalog loaded => CATALOG_UNAVAILABLE.
    let empty = CatalogCoordinator::new(Arc::new(MemorySource::new(None)), "edge-01", true);
    assert_eq!(
        empty.bundle_for_request(&json!({ "component": "opcua-adapter" }))["error"]["code"],
        "CATALOG_UNAVAILABLE"
    );
}

#[test]
fn update_from_message_bad_request_branches() {
    let coordinator = loaded_coordinator(true, true);

    for body in [
        json!("x"),                                   // not an object
        json!({ "catalog": {} }),                     // missing version
        json!({ "version": "v2" }),                   // missing catalog object
        json!({ "version": "v2", "catalog": "x" }),   // catalog not object
    ] {
        assert_eq!(coordinator.update_from_message(&body).ack["error"]["code"], "BAD_REQUEST");
    }

    // Version mismatch between envelope and catalog body.
    let mut cat = valid_catalog();
    cat["version"] = json!("v2");
    let result = coordinator.update_from_message(&json!({ "version": "vX", "catalog": cat }));
    assert_eq!(result.ack["error"]["code"], "CATALOG_INVALID");
}

#[test]
fn update_disabled_keeps_current_catalog() {
    let coordinator = loaded_coordinator(false, true);
    let mut cat = valid_catalog();
    cat["version"] = json!("v2");
    let result = coordinator.update_from_message(&json!({ "version": "v2", "catalog": cat }));
    assert_eq!(result.ack["error"]["code"], "CATALOG_UPDATE_DISABLED");
    assert_eq!(coordinator.active().unwrap().version, "v1");
    assert!(result.pushes.is_empty());
}

#[test]
fn reload_ignores_identical_fingerprint_and_rejects_invalid() {
    let coordinator = loaded_coordinator(false, true);

    // Same fingerprint as the active snapshot => ignored (no push).
    let same = SourceSnapshot {
        raw_catalog: valid_catalog(),
        version: "v1".into(),
        provenance: Map::new(),
        fingerprint: "fp".into(),
    };
    assert!(coordinator.reload_from_source_snapshot(same).is_empty());

    // A new but invalid snapshot is rejected and keeps the previous catalog.
    let invalid = SourceSnapshot {
        raw_catalog: json!({ "bogus": true }),
        version: "v2".into(),
        provenance: Map::new(),
        fingerprint: "fp2".into(),
    };
    assert!(coordinator.reload_from_source_snapshot(invalid).is_empty());
    assert_eq!(coordinator.active().unwrap().version, "v1");
}

#[test]
fn reload_from_source_promotes_and_optionally_pushes() {
    // push_on_reload = false: a valid reload promotes but does not push.
    let coordinator = loaded_coordinator(false, false);
    let mut next = valid_catalog();
    next["version"] = json!("v2");
    let snap = SourceSnapshot {
        raw_catalog: next,
        version: "v2".into(),
        provenance: Map::new(),
        fingerprint: "fp2".into(),
    };
    assert!(coordinator.reload_from_source_snapshot(snap).is_empty());
    assert_eq!(coordinator.active().unwrap().version, "v2");

    // push_on_reload = true: reload_from_source loads from the source and pushes.
    let coordinator = loaded_coordinator(false, true);
    // reload_from_source re-reads the MemorySource (same fingerprint) => ignored, empty.
    assert!(coordinator.reload_from_source().is_empty());
}

#[test]
fn pushes_for_without_active_catalog_is_empty() {
    let coordinator = CatalogCoordinator::new(Arc::new(MemorySource::new(None)), "edge-01", true);
    assert!(coordinator.pushes_for(None).is_empty());
    assert!(coordinator.watch_source().is_none());
}
