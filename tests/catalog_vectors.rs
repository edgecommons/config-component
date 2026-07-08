use std::sync::{Arc, Mutex};

use config_component::catalog::{Catalog, CatalogParseOptions, CATALOG_INVALID};
use config_component::coordinator::CatalogCoordinator;
use config_component::source::{CatalogSource, ReadOnlyCatalogSource, SourceSnapshot};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

fn vectors() -> Vec<Value> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../core/split-config-test-vectors/config-component-catalogs.json");
    let raw: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    raw["cases"].as_array().unwrap().clone()
}

fn vector_case(name: &str) -> Value {
    vectors()
        .into_iter()
        .find(|case| case["name"] == name)
        .unwrap_or_else(|| panic!("missing vector {name}"))
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
fn valid_catalog_with_base_and_two_components_vector() {
    let case = vector_case("valid-catalog-with-base-and-two-components");
    let catalog = Catalog::parse(
        case["input"]["catalog"].clone(),
        CatalogParseOptions::default(),
    )
    .unwrap();

    assert_eq!(catalog.version, case["expected"]["version"]);
    assert_eq!(
        catalog.components.len(),
        case["expected"]["componentCount"].as_u64().unwrap() as usize
    );
    assert_eq!(
        catalog.bundle_for("opcua-adapter").unwrap(),
        json!({
            "base": { "logging": { "level": "INFO" } },
            "component": { "component": { "token": "opcua-adapter" } }
        })
    );
}

#[test]
fn valid_catalog_with_no_base_vector() {
    let case = vector_case("valid-catalog-with-no-base");
    let catalog = Catalog::parse(
        case["input"]["catalog"].clone(),
        CatalogParseOptions::default(),
    )
    .unwrap();

    assert!(catalog.base.is_none());
    assert!(catalog.bundle_for("opcua-adapter").unwrap()["base"].is_null());
}

#[test]
fn catalog_version_provenance_present_vector() {
    let case = vector_case("catalog-version-provenance-present");
    let catalog = Catalog::parse(
        case["input"]["catalog"].clone(),
        CatalogParseOptions::default(),
    )
    .unwrap();

    assert_eq!(catalog.version, case["expected"]["version"]);
    assert_eq!(
        catalog.provenance["source"],
        case["expected"]["provenanceSource"]
    );
}

#[test]
fn file_loaded_catalog_derives_version_vector() {
    let case = vector_case("file-loaded-catalog-derives-version");
    let source = &case["input"]["source"];
    let catalog = Catalog::parse(
        case["input"]["catalog"].clone(),
        CatalogParseOptions {
            derived_version: Some(source["contentHash"].as_str().unwrap().to_string()),
            source_provenance: Some(Map::from_iter([
                ("source".to_string(), source["type"].clone()),
                ("uri".to_string(), source["uri"].clone()),
            ])),
            require_explicit_version: false,
        },
    )
    .unwrap();

    assert_eq!(catalog.version, source["contentHash"]);
    assert_eq!(case["expected"]["derivedVersionRequired"], true);
}

#[test]
fn invalid_catalog_vectors() {
    for name in [
        "invalid-missing-components",
        "invalid-non-object-component-entry",
    ] {
        let case = vector_case(name);
        let error = Catalog::parse(
            case["input"]["catalog"].clone(),
            CatalogParseOptions::default(),
        )
        .unwrap_err();
        assert_eq!(error.code, CATALOG_INVALID);
        assert_eq!(error.code, case["expected"]["error"].as_str().unwrap());
    }
}

#[test]
fn bad_request_missing_body_component_vector() {
    let case = vector_case("bad-request-missing-body-component");
    let coordinator = CatalogCoordinator::new(Arc::new(MemorySource::new(None)), "gw-01", true);

    let reply = coordinator.bundle_for_request(&case["input"]["requestBody"]);

    assert_eq!(reply["ok"], false);
    assert_eq!(
        reply["error"]["code"],
        case["expected"]["reply"]["error"]["code"]
    );
}

#[test]
fn not_found_unknown_component_token_vector() {
    let case = vector_case("not-found-unknown-component-token");
    let coordinator = CatalogCoordinator::new(
        Arc::new(MemorySource::new(Some(case["input"]["catalog"].clone()))),
        "gw-01",
        true,
    );
    assert!(coordinator.load_initial());

    let reply = coordinator.bundle_for_request(&case["input"]["requestBody"]);

    assert_eq!(reply["ok"], false);
    assert_eq!(
        reply["error"]["code"],
        case["expected"]["reply"]["error"]["code"]
    );
}

#[test]
fn catalog_reload_push_bundle_for_every_component_vector() {
    let case = vector_case("catalog-reload-push-bundle-for-every-component");
    let coordinator = CatalogCoordinator::new(
        Arc::new(MemorySource::new(Some(case["input"]["oldCatalog"].clone()))),
        "gw-01",
        true,
    );
    assert!(coordinator.load_initial());

    let mut snap = snapshot(case["input"]["newCatalog"].clone());
    snap.fingerprint = "new".to_string();
    let pushes = coordinator.reload_from_source_snapshot(snap);

    let topics = pushes
        .iter()
        .map(|push| push.topic.as_str())
        .collect::<Vec<_>>();
    let expected = case["expected"]["pushTopics"]
        .as_array()
        .unwrap()
        .iter()
        .map(Value::as_str)
        .map(Option::unwrap)
        .collect::<Vec<_>>();
    assert_eq!(topics, expected);
    assert!(pushes
        .iter()
        .all(|push| push.version == case["expected"]["pushedVersion"]));
    assert_eq!(
        pushes[0].body["base"],
        json!({ "logging": { "level": "WARN" } })
    );
}

#[test]
fn message_update_valid_full_replacement_vector() {
    let case = vector_case("message-update-valid-full-replacement");
    let source = Arc::new(MemorySource::new(Some(json!({
        "schemaVersion": 1,
        "version": "old",
        "components": {}
    }))));
    let coordinator =
        CatalogCoordinator::with_volatile_updates(source.clone(), "gw-01", true, true);
    assert!(coordinator.load_initial());

    let result = coordinator.update_from_message(&case["input"]["updateBody"]);

    assert_eq!(result.ack["ok"], true);
    assert_eq!(result.ack["version"], case["expected"]["ack"]["version"]);
    assert_eq!(result.ack["provenance"]["source"], "message");
    assert_eq!(result.ack["provenance"]["volatile"], true);
    assert_eq!(
        coordinator.active().unwrap().version,
        case["expected"]["ack"]["version"].as_str().unwrap()
    );
}

#[test]
fn message_update_disabled_by_default_rejects_and_keeps_current_vector() {
    let case = vector_case("message-update-disabled-by-default-rejects-and-keeps-current");
    let coordinator = CatalogCoordinator::new(
        Arc::new(MemorySource::new(Some(json!({
            "schemaVersion": 1,
            "version": case["input"]["activeVersion"].clone(),
            "components": {}
        })))),
        "gw-01",
        true,
    );
    assert!(coordinator.load_initial());

    let result = coordinator.update_from_message(&case["input"]["updateBody"]);

    assert_eq!(
        result.ack["error"]["code"],
        case["expected"]["ack"]["error"]["code"]
    );
    assert_eq!(
        coordinator.active().unwrap().version,
        case["expected"]["activeVersion"].as_str().unwrap()
    );
    assert!(result.pushes.is_empty());
}

#[test]
fn message_update_invalid_catalog_rejects_and_keeps_current_vector() {
    let case = vector_case("message-update-invalid-catalog-rejects-and-keeps-current");
    let coordinator = CatalogCoordinator::with_volatile_updates(
        Arc::new(MemorySource::new(Some(json!({
            "schemaVersion": 1,
            "version": case["input"]["activeVersion"].clone(),
            "components": {}
        })))),
        "gw-01",
        true,
        true,
    );
    assert!(coordinator.load_initial());

    let result = coordinator.update_from_message(&case["input"]["updateBody"]);

    assert_eq!(
        result.ack["error"]["code"],
        case["expected"]["ack"]["error"]["code"]
    );
    assert_eq!(
        coordinator.active().unwrap().version,
        case["expected"]["activeVersion"].as_str().unwrap()
    );
    assert!(result.pushes.is_empty());
}

#[test]
fn read_only_source_accepts_enabled_volatile_message_update_vector() {
    let case = vector_case("read-only-source-accepts-enabled-volatile-message-update");
    let coordinator = CatalogCoordinator::with_volatile_updates(
        Arc::new(ReadOnlyCatalogSource::new(snapshot(json!({
            "schemaVersion": 1,
            "version": "old",
            "components": {}
        })))),
        "gw-01",
        true,
        true,
    );
    assert!(coordinator.load_initial());

    let result = coordinator.update_from_message(&case["input"]["updateBody"]);

    assert_eq!(result.ack["ok"], true);
    assert_eq!(result.ack["version"], case["expected"]["ack"]["version"]);
    assert_eq!(coordinator.active().unwrap().version, "new");
}
