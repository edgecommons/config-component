use std::sync::{Arc, Mutex};

use config_component::catalog::{Catalog, CatalogParseOptions};
use config_component::coordinator::CatalogCoordinator;
use config_component::source::{CatalogSource, ReadOnlyCatalogSource, SourceSnapshot};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

fn vectors() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../core/hierarchical-config-test-vectors/catalogs.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn valid_case(name: &str) -> Value {
    vectors()["validCatalogs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("missing valid catalog vector {name}"))
}

fn invalid_case(name: &str) -> Value {
    vectors()["invalidCatalogs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("missing invalid catalog vector {name}"))
}

fn fingerprint(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap();
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn snapshot(raw: Value) -> SourceSnapshot {
    let version = raw
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| fingerprint(&raw));
    SourceSnapshot {
        raw_catalog: raw,
        version,
        provenance: Map::new(),
        fingerprint: "memory".to_string(),
    }
}

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
        Ok(snapshot(raw))
    }

    fn watch(&self) -> Option<mpsc::UnboundedReceiver<SourceSnapshot>> {
        None
    }
}

#[test]
fn valid_four_layer_catalog_serves_lineage_bundle() {
    let case = valid_case("four-layer-line-with-one-device");
    let catalog = Catalog::parse(case["catalog"].clone(), CatalogParseOptions::default()).unwrap();

    assert_eq!(catalog.version, case["expected"]["version"]);
    assert_eq!(
        catalog.components.len(),
        case["expected"]["componentCount"].as_u64().unwrap() as usize
    );

    let body = catalog.lineage_for("opcua-adapter").unwrap();
    assert_eq!(body["lineageVersion"], 1);
    assert_eq!(body["catalogVersion"], case["expected"]["version"]);
    assert_eq!(body["component"], "opcua-adapter");
    assert!(body.get("base").is_none());
    let ids = body["layers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|layer| layer["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    let expected = case["expected"]["lineageIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(Value::as_str)
        .map(Option::unwrap)
        .collect::<Vec<_>>();
    assert_eq!(ids, expected);
}

#[test]
fn valid_component_only_catalog_serves_one_layer() {
    let case = valid_case("component-only-lineage");
    let catalog = Catalog::parse(case["catalog"].clone(), CatalogParseOptions::default()).unwrap();

    let body = catalog.lineage_for("opcua-adapter").unwrap();
    assert_eq!(body["layers"].as_array().unwrap().len(), 1);
    assert_eq!(body["layers"][0]["id"], "component/opcua-adapter");
}

#[test]
fn invalid_catalog_vectors_return_expected_codes() {
    for name in [
        "old-split-catalog-with-base-is-invalid",
        "missing-hierarchy",
        "duplicate-hierarchy-level",
        "device-in-catalog-scope",
        "node-missing-scope",
        "node-scope-does-not-own-node-id",
        "unknown-parent",
        "unreferenced-node-unknown-parent",
        "cycle",
        "unreferenced-node-cycle",
        "scope-conflict",
        "identity-conflict",
    ] {
        let case = invalid_case(name);
        let error =
            Catalog::parse(case["catalog"].clone(), CatalogParseOptions::default()).unwrap_err();
        assert_eq!(
            error.code,
            case["expected"]["error"].as_str().unwrap(),
            "{name}"
        );
    }
}

#[test]
fn bad_request_missing_body_component_returns_error_body() {
    let coordinator =
        CatalogCoordinator::new(Arc::new(MemorySource::new(None)), "line-7-edge-01", true);

    let reply = coordinator.bundle_for_request(&json!({}));

    assert_eq!(reply["ok"], false);
    assert_eq!(reply["error"]["code"], "BAD_REQUEST");
}

#[test]
fn unknown_component_token_returns_not_found() {
    let case = valid_case("four-layer-line-with-one-device");
    let coordinator = CatalogCoordinator::new(
        Arc::new(MemorySource::new(Some(case["catalog"].clone()))),
        "line-7-edge-01",
        true,
    );
    assert!(coordinator.load_initial());

    let reply = coordinator.bundle_for_request(&json!({ "component": "missing" }));

    assert_eq!(reply["ok"], false);
    assert_eq!(reply["error"]["code"], "CONFIG_NOT_FOUND");
}

#[test]
fn catalog_reload_pushes_lineage_for_every_component() {
    let initial = valid_case("four-layer-line-with-one-device");
    let reload = valid_case("reload-relocates-line-and-pushes-all-components");
    let coordinator = CatalogCoordinator::new(
        Arc::new(MemorySource::new(Some(initial["catalog"].clone()))),
        "line-7-edge-01",
        true,
    );
    assert!(coordinator.load_initial());

    let mut snap = snapshot(reload["newCatalog"].clone());
    snap.fingerprint = "new".to_string();
    let pushes = coordinator.reload_from_source_snapshot(snap);

    assert_eq!(
        pushes.len(),
        reload["expected"]["pushCount"].as_u64().unwrap() as usize
    );
    assert!(pushes
        .iter()
        .all(|push| push.version == "enterprise-site-zone-line-v2"));
    assert!(pushes.iter().all(|push| push.body["lineageVersion"] == 1));
    assert!(pushes.iter().all(|push| push.body.get("base").is_none()));
}

#[test]
fn message_update_valid_full_replacement_is_volatile_and_pushes_lineages() {
    let initial = valid_case("four-layer-line-with-one-device");
    let reload = valid_case("reload-relocates-line-and-pushes-all-components");
    let source = Arc::new(MemorySource::new(Some(initial["catalog"].clone())));
    let coordinator =
        CatalogCoordinator::with_volatile_updates(source.clone(), "line-7-edge-01", true, true);
    assert!(coordinator.load_initial());

    let version = reload["newCatalog"]["version"].as_str().unwrap();
    let result = coordinator.update_from_message(&json!({
        "version": version,
        "catalog": reload["newCatalog"].clone()
    }));

    assert_eq!(result.ack["ok"], true);
    assert_eq!(result.ack["version"], version);
    assert_eq!(result.ack["provenance"]["source"], "message");
    assert_eq!(result.ack["provenance"]["volatile"], true);
    assert_eq!(result.pushes.len(), 2);
    assert!(result
        .pushes
        .iter()
        .all(|push| push.body["lineageVersion"] == 1));
    assert_eq!(coordinator.active().unwrap().version, version);
}

#[test]
fn message_update_overrides_payload_provenance_with_volatile_message_provenance() {
    let initial = valid_case("four-layer-line-with-one-device");
    let reload = valid_case("reload-relocates-line-and-pushes-all-components");
    let source = Arc::new(MemorySource::new(Some(initial["catalog"].clone())));
    let coordinator =
        CatalogCoordinator::with_volatile_updates(source.clone(), "line-7-edge-01", true, true);
    assert!(coordinator.load_initial());

    let mut catalog = reload["newCatalog"].clone();
    catalog["provenance"] = json!({
        "source": "configmap",
        "uri": "configmap://stale-copied-payload",
        "volatile": false
    });
    let version = catalog["version"].as_str().unwrap();
    let result = coordinator.update_from_message(&json!({
        "version": version,
        "catalog": catalog
    }));

    assert_eq!(result.ack["ok"], true);
    assert_eq!(result.ack["provenance"]["source"], "message");
    assert_eq!(result.ack["provenance"]["interface"], "update-catalog");
    assert_eq!(result.ack["provenance"]["volatile"], true);
    assert!(result.pushes.iter().all(|push| {
        push.body["provenance"]["source"] == "message"
            && push.body["provenance"]["interface"] == "update-catalog"
            && push.body["provenance"]["volatile"] == true
    }));
    let active = coordinator.active().unwrap();
    assert_eq!(active.provenance["source"], "message");
    assert_eq!(active.raw["provenance"]["source"], "message");
}

#[test]
fn message_update_disabled_by_default_rejects_and_keeps_current() {
    let initial = valid_case("four-layer-line-with-one-device");
    let reload = valid_case("reload-relocates-line-and-pushes-all-components");
    let coordinator = CatalogCoordinator::new(
        Arc::new(MemorySource::new(Some(initial["catalog"].clone()))),
        "line-7-edge-01",
        true,
    );
    assert!(coordinator.load_initial());

    let result = coordinator.update_from_message(&json!({
        "version": reload["newCatalog"]["version"].as_str().unwrap(),
        "catalog": reload["newCatalog"].clone()
    }));

    assert_eq!(result.ack["ok"], false);
    assert_eq!(result.ack["error"]["code"], "CATALOG_UPDATE_DISABLED");
    assert_eq!(
        coordinator.active().unwrap().version,
        initial["catalog"]["version"]
    );
    assert!(result.pushes.is_empty());
}

#[test]
fn invalid_message_update_rejects_and_keeps_current() {
    let initial = valid_case("four-layer-line-with-one-device");
    let invalid = invalid_case("old-split-catalog-with-base-is-invalid");
    let coordinator = CatalogCoordinator::with_volatile_updates(
        Arc::new(MemorySource::new(Some(initial["catalog"].clone()))),
        "line-7-edge-01",
        true,
        true,
    );
    assert!(coordinator.load_initial());

    let result = coordinator.update_from_message(&json!({
        "version": "bad",
        "catalog": invalid["catalog"].clone()
    }));

    assert_eq!(result.ack["ok"], false);
    assert_eq!(result.ack["error"]["code"], "CATALOG_INVALID");
    assert_eq!(
        coordinator.active().unwrap().version,
        initial["catalog"]["version"]
    );
    assert!(result.pushes.is_empty());
}

#[test]
fn read_only_source_accepts_enabled_volatile_message_update() {
    let initial = valid_case("four-layer-line-with-one-device");
    let reload = valid_case("reload-relocates-line-and-pushes-all-components");
    let coordinator = CatalogCoordinator::with_volatile_updates(
        Arc::new(ReadOnlyCatalogSource::new(snapshot(
            initial["catalog"].clone(),
        ))),
        "line-7-edge-01",
        true,
        true,
    );
    assert!(coordinator.load_initial());

    let version = reload["newCatalog"]["version"].as_str().unwrap();
    let result = coordinator.update_from_message(&json!({
        "version": version,
        "catalog": reload["newCatalog"].clone()
    }));

    assert_eq!(result.ack["ok"], true);
    assert_eq!(result.ack["version"], version);
    assert_eq!(coordinator.active().unwrap().version, version);
}
