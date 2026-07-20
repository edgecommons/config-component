# Reference — Configuration

`com.mbreissi.edgecommons.ConfigComponent` is the built-in server role for `CONFIG_COMPONENT`
hierarchical-config deployments. It is implemented in Rust. Ordinary EdgeCommons components remain
clients. This page documents the server's own bootstrap config, its catalog source descriptors, and the
catalog format it loads. `config.schema.json` is the machine-readable contract for the
`component.global` object described here.

## Bootstrap

The component starts from `GG_CONFIG`, `FILE`, `ENV`, or `CONFIGMAP`. Launching it with
`-c CONFIG_COMPONENT` is rejected before subscriptions are created, so the server can never bootstrap
from itself. On Greengrass, the recipe uses the platform default `GG_CONFIG` and reads `ComponentConfig`.

The bootstrap config is the ConfigComponent's own effective config. It is not a catalog entry and is
never returned to other components. Its own settings live under `component.global.configComponent`:

| Key | Type | Default | Meaning |
|-----|------|---------|---------|
| `component.token` | string | — | The component token; `edgecommons-config-component`. |
| `component.global.configComponent.catalogSource` | object | — | The catalog source descriptor (see below). Required. |
| `component.global.configComponent.pushOnCatalogReload` | boolean | `true` | Push a complete `set-config` lineage bundle to every catalog component when a new catalog is promoted. |
| `component.global.configComponent.allowVolatileCatalogUpdates` | boolean | `false` | Enable the `update-catalog` message interface (debug/verification/test only). |

`component.instances[]` is always empty — the ConfigComponent reads no per-instance config.

## Catalog source

Catalog loading goes through the `CatalogSource` seam. The component supports a local JSON file, a
Kubernetes ConfigMap-mounted file, and an environment-variable catalog source.

```json
{
  "type": "file",
  "path": "/greengrass/v2/work/com.mbreissi.edgecommons.ConfigComponent/catalog.json",
  "watch": true
}
```

```json
{
  "type": "configmap",
  "mountDir": "/etc/edgecommons",
  "key": "catalog.json",
  "watch": true
}
```

```json
{
  "type": "env",
  "var": "EDGECOMMONS_CONFIG_CATALOG"
}
```

| Field | Applies to | Meaning |
|-------|-----------|---------|
| `type` | all | `file`, `configmap`, or `env`. Required. |
| `path` | file, configmap | For `file`, the required, non-empty catalog path. For `configmap`, the full mounted-file path; when present it wins over `mountDir`/`key`. |
| `mountDir` | configmap | ConfigMap mount directory. Defaults to `/etc/edgecommons`. Used with `key` when `path` is absent. |
| `key` | configmap | ConfigMap data key — a bare file name, not a path — joined onto `mountDir`. Defaults to `catalog.json`. |
| `var` | env | Environment variable holding inline catalog JSON or `@/path/to/catalog.json`. Defaults to `EDGECOMMONS_CONFIG_CATALOG`. |
| `watch` | file, configmap | Poll for content changes and hot-promote a new catalog. Defaults to `false`. Environment sources are snapshot-only and reject `watch`. |

If `version` is absent in a source-loaded catalog, the server derives `version` from a `sha256:` content
hash. If `provenance` is absent, it derives `source`, `uri`, and `contentHash` from the source
descriptor.

ConfigMap sources are read/watch only. A ConfigMap change is applied by Kubernetes, observed through the
mounted file, and promoted by the ConfigComponent. The component does not write back to the ConfigMap.

## Catalog format

```json
{
  "schemaVersion": 1,
  "version": "2026-07-07T18:30:00Z",
  "provenance": { "source": "file", "uri": "/etc/edgecommons/catalog.json" },
  "hierarchy": { "levels": ["enterprise", "site", "zone", "line", "device"] },
  "nodes": {
    "enterprise/acme": {
      "scope": { "enterprise": "acme" },
      "config": {
        "hierarchy": { "levels": ["enterprise", "site", "zone", "line", "device"] },
        "identity": { "enterprise": "acme" },
        "logging": { "level": "INFO" }
      }
    },
    "site/dallas": {
      "parent": "enterprise/acme",
      "scope": { "enterprise": "acme", "site": "dallas" },
      "config": { "identity": { "site": "dallas" } }
    },
    "zone/packaging": {
      "parent": "site/dallas",
      "scope": { "enterprise": "acme", "site": "dallas", "zone": "packaging" },
      "config": { "identity": { "zone": "packaging" } }
    },
    "line/line-7": {
      "parent": "zone/packaging",
      "scope": { "enterprise": "acme", "site": "dallas", "zone": "packaging", "line": "line-7" },
      "config": { "identity": { "line": "line-7" } }
    }
  },
  "components": {
    "opcua-adapter": {
      "parent": "line/line-7",
      "config": { "component": { "token": "opcua-adapter", "instances": [] } }
    }
  }
}
```

`hierarchy.levels` is a non-empty ordered list ending in `device`. Catalog `nodes` describe shared scopes
above the runtime device; the device itself is resolved by the platform and is not stored as a catalog
node. Node ids are `<level>/<value>`, each node has at most one `parent`, and `scope` is a non-empty
object that grows monotonically down the lineage and includes the node id's own `<level>: <value>` claim.
Components are keyed by sanitized short component lookup token and may point at any node.

Node and component `config` values are raw partial config layers. The server validates catalog structure,
lineage parentage, scope monotonicity, and identity ownership, but it does not schema-validate a raw layer
as an effective component config. Consumers merge `layers[].config` locally and validate the merged result.

`schemaVersion` is `1`. A message-delivered catalog includes a non-empty `version`; source-loaded catalogs
may derive one from content. Old split catalogs with a top-level `base` are invalid.

## Build and packaging

Local development uses the default `standalone` feature. `Cargo.toml` pins the `edgecommons` library by
git rev; a gitignored `.cargo/config.toml` `[patch]` block points it at a sibling checkout for local dev.

```bash
cargo test
cargo build
```

Greengrass packaging uses `build.sh`, which builds the binary with `--no-default-features --features
greengrass` and stages the artifact expected by `recipe.yaml`. The `greengrass` feature is Linux-only.
