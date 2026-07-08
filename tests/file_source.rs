use std::time::Duration;

use std::sync::Arc;

use config_component::catalog::CATALOG_UPDATE_DISABLED;
use config_component::coordinator::CatalogCoordinator;
use config_component::source::{
    source_from_descriptor, CatalogSource, ConfigMapCatalogSource, FileCatalogSource,
};
use serde_json::json;

#[test]
fn file_source_loads_and_derives_version_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.json");
    std::fs::write(
        &path,
        r#"{ "schemaVersion": 1, "components": { "opcua-adapter": { "component": { "token": "opcua-adapter" } } } }"#,
    )
    .unwrap();

    let source = FileCatalogSource::new(&path, false);
    let snapshot = source.load().unwrap();

    assert!(snapshot.version.starts_with("sha256:"));
    assert_eq!(snapshot.provenance["source"], "file");
    assert_eq!(snapshot.raw_catalog["schemaVersion"], 1);
}

#[tokio::test]
async fn file_source_watch_polls_for_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.json");
    std::fs::write(
        &path,
        r#"{ "schemaVersion": 1, "version": "old", "components": {} }"#,
    )
    .unwrap();

    let source = FileCatalogSource::with_poll_interval(&path, true, Duration::from_millis(25));
    let mut rx = source.watch().unwrap();
    std::fs::write(
        &path,
        r#"{ "schemaVersion": 1, "version": "new", "components": {} }"#,
    )
    .unwrap();

    let snapshot = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.version, "new");
}

#[test]
fn descriptor_builds_file_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.json");
    std::fs::write(
        &path,
        r#"{ "schemaVersion": 1, "version": "file", "components": {} }"#,
    )
    .unwrap();

    let source = source_from_descriptor(&json!({
        "type": "file",
        "path": path.to_string_lossy(),
        "watch": true
    }))
    .unwrap();
    assert_eq!(source.load().unwrap().version, "file");
}

#[test]
fn configmap_source_loads_as_read_only_with_configmap_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.json");
    std::fs::write(
        &path,
        r#"{ "schemaVersion": 1, "components": { "opcua-adapter": { "component": { "token": "opcua-adapter" } } } }"#,
    )
    .unwrap();

    let source = ConfigMapCatalogSource::new(&path, false);
    let snapshot = source.load().unwrap();

    assert!(snapshot.version.starts_with("sha256:"));
    assert_eq!(snapshot.provenance["source"], "configmap");
    assert_eq!(snapshot.raw_catalog["schemaVersion"], 1);
}

#[test]
fn descriptor_builds_configmap_source_from_mount_dir_and_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.json");
    std::fs::write(
        &path,
        r#"{ "schemaVersion": 1, "version": "mounted", "components": {} }"#,
    )
    .unwrap();

    let source = source_from_descriptor(&json!({
        "type": "configmap",
        "mountDir": dir.path().to_string_lossy(),
        "key": "catalog.json",
        "watch": false
    }))
    .unwrap();

    let snapshot = source.load().unwrap();
    assert_eq!(snapshot.version, "mounted");
    assert_eq!(snapshot.provenance["source"], "configmap");
}

#[tokio::test]
async fn configmap_source_watch_polls_for_mounted_file_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.json");
    std::fs::write(
        &path,
        r#"{ "schemaVersion": 1, "version": "old", "components": {} }"#,
    )
    .unwrap();

    let source = ConfigMapCatalogSource::with_poll_interval(&path, true, Duration::from_millis(25));
    let mut rx = source.watch().unwrap();
    std::fs::write(
        &path,
        r#"{ "schemaVersion": 1, "version": "new", "components": {} }"#,
    )
    .unwrap();

    let snapshot = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.version, "new");
}

#[test]
fn update_catalog_disabled_by_default_does_not_overwrite_configmap_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.json");
    let original_catalog = json!({
        "schemaVersion": 1,
        "version": "old",
        "components": {}
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&original_catalog).unwrap()).unwrap();
    let original_bytes = std::fs::read(&path).unwrap();

    let source = source_from_descriptor(&json!({
        "type": "configmap",
        "path": path.to_string_lossy(),
        "watch": false
    }))
    .unwrap();
    let source: Arc<dyn CatalogSource> = Arc::from(source);
    let coordinator = CatalogCoordinator::new(source, "gw-01", true);
    assert!(coordinator.load_initial());

    let result = coordinator.update_from_message(&json!({
        "version": "new",
        "catalog": {
            "schemaVersion": 1,
            "version": "new",
            "components": {
                "opcua-adapter": { "component": { "token": "opcua-adapter" } }
            }
        }
    }));

    assert_eq!(result.ack["error"]["code"], CATALOG_UPDATE_DISABLED);
    assert!(result.pushes.is_empty());
    assert_eq!(coordinator.active().unwrap().version, "old");
    assert_eq!(std::fs::read(&path).unwrap(), original_bytes);
}

#[test]
fn volatile_update_catalog_can_override_configmap_cache_without_overwriting_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.json");
    let original_catalog = json!({
        "schemaVersion": 1,
        "version": "old",
        "components": {}
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&original_catalog).unwrap()).unwrap();
    let original_bytes = std::fs::read(&path).unwrap();

    let source = source_from_descriptor(&json!({
        "type": "configmap",
        "path": path.to_string_lossy(),
        "watch": false
    }))
    .unwrap();
    let source: Arc<dyn CatalogSource> = Arc::from(source);
    let coordinator = CatalogCoordinator::with_volatile_updates(source, "gw-01", true, true);
    assert!(coordinator.load_initial());

    let result = coordinator.update_from_message(&json!({
        "version": "new",
        "catalog": {
            "schemaVersion": 1,
            "version": "new",
            "components": {
                "opcua-adapter": { "component": { "token": "opcua-adapter" } }
            }
        }
    }));

    assert_eq!(result.ack["ok"], true);
    assert_eq!(result.ack["version"], "new");
    assert_eq!(result.ack["provenance"]["source"], "message");
    assert_eq!(result.ack["provenance"]["volatile"], true);
    assert_eq!(coordinator.active().unwrap().version, "new");
    assert_eq!(std::fs::read(&path).unwrap(), original_bytes);
}
