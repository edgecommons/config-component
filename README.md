# EdgeCommons ConfigComponent

Dedicated deployable configuration catalog server for
`com.mbreissi.edgecommons.ConfigComponent`, implemented as a Rust binary.

It bootstraps from the component's own non-`CONFIG_COMPONENT` config source, loads a catalog from
`component.global.configComponent.catalogSource`, and manually serves the reserved
`CONFIG_COMPONENT` rendezvous:

- `ecv1/{device}/config/main/cmd/get-configuration`
- `ecv1/{device}/config/main/cmd/update-catalog`

Successful GET replies are raw layer bundles:

```json
{
  "base": { "logging": { "level": "INFO" } },
  "component": { "component": { "token": "opcua-adapter" } }
}
```

The server does not merge layers. Clients merge and validate their own effective config.

## Run Locally

Start a local MQTT broker, then run:

```bash
cargo run -- \
  --platform HOST \
  --transport MQTT test-configs/standalone-messaging.json \
  -c FILE test-configs/config.json \
  -t gw-01
```

The crate depends on the sibling Rust library through a path dependency:

`edgecommons = { path = "../core/libs/rust", default-features = false }`

## Catalog Source

v1 supports JSON catalogs from a local file or a Kubernetes ConfigMap-mounted file:

```json
{
  "component": {
    "token": "edgecommons-config-component",
    "global": {
      "configComponent": {
        "catalogSource": {
          "type": "file",
          "path": "/greengrass/v2/work/com.mbreissi.edgecommons.ConfigComponent/catalog.json",
          "watch": true
        },
        "pushOnCatalogReload": true,
        "allowVolatileCatalogUpdates": false
      }
    }
  }
}
```

Supported source descriptors:

```json
{ "type": "file", "path": "/path/to/catalog.json", "watch": true }
```

```json
{ "type": "configmap", "path": "/etc/edgecommons/catalog.json", "watch": true }
```

```json
{ "type": "configmap", "mountDir": "/etc/edgecommons", "key": "catalog.json", "watch": true }
```

File and ConfigMap-loaded catalogs may omit `version` and `provenance`; the source derives them
from the content hash and path. ConfigMap is read/watch only. Kubernetes updates the ConfigMap; the
ConfigComponent observes the mounted file change, updates its active cache, and serves the new
catalog. The component does not write back to a ConfigMap.

Message updates are complete catalog replacements delivered to
`ecv1/{device}/config/main/cmd/update-catalog`. The request body contains `version` and `catalog`,
and the two versions must match. This interface is disabled by default and is intended only for
debug, verification, and test environments. Enable it with:

```json
{ "allowVolatileCatalogUpdates": true }
```

When enabled, the component validates the replacement, promotes it only to the active in-memory
cache, acknowledges with `{"ok":true,"version":...}`, and pushes complete `set-config` bundles when
configured. It never writes message-delivered catalogs to the file or ConfigMap source, so the
override does not survive restart. Invalid or disabled updates return
`{"ok":false,"error":{"code":...,"message":...}}` and keep the previous active catalog.

## Greengrass Build

The GDK custom build script builds a Linux Greengrass artifact with the `greengrass` feature:

```bash
gdk component build
```

On Windows, build and test the default standalone feature locally with `cargo test` and `cargo build`.
The Greengrass IPC feature remains Linux-only because the Greengrass IPC SDK is Linux-only.
