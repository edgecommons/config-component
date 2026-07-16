# ConfigComponent Reference

`com.mbreissi.edgecommons.ConfigComponent` is the built-in server role for
`CONFIG_COMPONENT` hierarchical-config deployments. It is implemented in Rust. Ordinary EdgeCommons
components remain clients.

## Bootstrap

The component must start from `GG_CONFIG`, `FILE`, `ENV`, or `CONFIGMAP`. Launching it with
`-c CONFIG_COMPONENT` is rejected before subscriptions are created. On Greengrass, the recipe uses
the platform default `GG_CONFIG` and reads `ComponentConfig`.

Required bootstrap config:

- `component.token`: should be `edgecommons-config-component`
- `component.global.configComponent.catalogSource`: `file`, `configmap`, or `env` source descriptor
- `component.global.configComponent.pushOnCatalogReload`: optional boolean, default `true`
- `component.global.configComponent.allowVolatileCatalogUpdates`: optional boolean, default `false`

The bootstrap config is the ConfigComponent's own effective config. It is not a catalog entry and is
not returned to other components.

## Catalog Source

Catalog loading goes through the `CatalogSource` seam. The component supports local JSON file,
Kubernetes ConfigMap-mounted file, and environment-variable catalog sources:

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

`file.path` is required and non-empty. `configmap` may use either `path`, or `mountDir` plus `key`;
`key` must be a file name inside the mount. `env.var` is optional and defaults to
`EDGECOMMONS_CONFIG_CATALOG`. `watch` is optional (`false` by default) for file and ConfigMap
sources; environment sources are snapshot-only. If `version` is absent in a source-loaded catalog,
the server derives `version` from a `sha256:` content hash. If `provenance` is absent, it derives
`source`, `uri`, and `contentHash` from the source descriptor.

ConfigMap sources are read/watch only. A ConfigMap change is applied by Kubernetes, observed through
the mounted file, and promoted by the ConfigComponent. The component does not write back to the
ConfigMap.

## Catalog Format

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
      "scope": {
        "enterprise": "acme",
        "site": "dallas",
        "zone": "packaging",
        "line": "line-7"
      },
      "config": { "identity": { "line": "line-7" } }
    }
  },
  "components": {
    "opcua-adapter": {
      "parent": "line/line-7",
      "config": {
        "component": { "token": "opcua-adapter", "instances": [] }
      }
    }
  }
}
```

`hierarchy.levels` must be a non-empty ordered list ending in `device`. Catalog `nodes` describe
shared scopes above the runtime device. The device itself is resolved by the platform and is not
stored as a catalog node. Node ids are `<level>/<value>`, each node has at most one `parent`, and
`scope` must be a non-empty object that grows monotonically down the lineage and includes the node
id's own `<level>:<value>` claim. Components are keyed by sanitized short component lookup token and
may point at any node.

Node and component `config` values are raw partial config layers. The server validates catalog
structure, lineage parentage, scope monotonicity, and identity ownership, but it does not
schema-validate a raw layer as an effective component config. Consumers merge `layers[].config`
locally and validate the merged result.

`schemaVersion` must be `1`. A message-delivered catalog must include a non-empty `version`;
source-loaded catalogs may derive one from content. Old split catalogs with a top-level `base` are
invalid.

## Request And Update Topics

- GET: `ecv1/{device}/config/cmd/get-configuration`
- update: `ecv1/{device}/config/cmd/update-catalog`
- push: `ecv1/{device}/{component}/cmd/set-config`

All messages use the normal EdgeCommons message envelope. The server reads a raw JSON body when one
is present, otherwise it reads the structured message body. If a request has `reply_to`, the server
replies; otherwise it processes the request without an acknowledgement.

GET requires body:

```json
{ "component": "opcua-adapter" }
```

Successful GET replies are lineage bundles:

```json
{
  "lineageVersion": 1,
  "catalogVersion": "2026-07-07T18:30:00Z",
  "component": "opcua-adapter",
  "provenance": { "source": "file", "uri": "/etc/edgecommons/catalog.json" },
  "layers": [
    {
      "id": "enterprise/acme",
      "kind": "scope",
      "scope": { "enterprise": "acme" },
      "config": {
        "hierarchy": { "levels": ["enterprise", "site", "zone", "line", "device"] },
        "identity": { "enterprise": "acme" }
      }
    },
    {
      "id": "component/opcua-adapter",
      "kind": "component",
      "component": "opcua-adapter",
      "config": { "component": { "token": "opcua-adapter" } }
    }
  ]
}
```

`layers` is ordered from the highest shared scope to the requested component. A valid client merges
only `layers[].config`. It rejects legacy component-only documents and old `{base, component}`
bundles.

Unknown components and bad requests return:

```json
{
  "ok": false,
  "error": {
    "code": "CONFIG_NOT_FOUND",
    "message": "No configuration catalog entry for component 'opcua-adapter'"
  }
}
```

`update-catalog` is a non-production debug/verification/test interface. It is disabled by default;
enable it only in non-prod with `component.global.configComponent.allowVolatileCatalogUpdates:true`.
Update requires a complete replacement. The top-level `version` must match `catalog.version`:

```json
{
  "version": "git:8f2c9c7",
  "catalog": {
      "schemaVersion": 1,
      "version": "git:8f2c9c7",
      "hierarchy": { "levels": ["enterprise", "site", "zone", "line", "device"] },
      "nodes": {},
      "components": {
        "opcua-adapter": {
          "config": {
            "hierarchy": { "levels": ["enterprise", "site", "zone", "line", "device"] },
            "identity": {
              "enterprise": "acme",
              "site": "dallas",
              "zone": "packaging",
              "line": "line-7"
            },
            "component": { "token": "opcua-adapter" }
          }
        }
      }
    }
  }
```

Successful update replies are `CatalogUpdateAck` bodies:

```json
{
  "ok": true,
  "version": "git:8f2c9c7",
  "provenance": { "source": "message", "interface": "update-catalog", "volatile": true }
}
```

On a valid enabled update, the server promotes the replacement only to the active in-memory cache,
replies with the promoted version/provenance, and publishes lineage bundles to every component in
the catalog when `pushOnCatalogReload` is `true`. It never writes the replacement to the active file
or ConfigMap source. The override is gone after restart or after a later source-side reload replaces
it. Disabled updates, invalid updates, and invalid source reloads keep the previous active catalog
and do not push.

## Error Codes

- `BAD_REQUEST`: request body is not an object or lacks a required string/object field.
- `CONFIG_NOT_FOUND`: the requested component token is absent from the active catalog.
- `CATALOG_UNAVAILABLE`: no valid catalog is currently loaded.
- `CATALOG_INVALID`: catalog shape, schema version, hierarchy, node ids, component keys, version matching, or layer shape is invalid.
- `CATALOG_UPDATE_DISABLED`: volatile message updates are disabled.
- `LINEAGE_CYCLE`: a component lineage revisits a node.
- `LINEAGE_PARENT_MISSING`: a node or component references a missing parent.
- `LINEAGE_DEPTH_EXCEEDED`: a lineage exceeds the maximum supported depth.
- `LINEAGE_SCOPE_CONFLICT`: a child scope overwrites an ancestor scope value.
- `LINEAGE_IDENTITY_CONFLICT`: a layer identity overwrites an ancestor-owned identity value.

Failures keep the previous active catalog and do not push `set-config`.

## Greengrass Permissions

The recipe grants the server only the protocol topics it needs. These are Greengrass IPC authorization
resource patterns, so they use Greengrass `*` matching instead of MQTT `+` or `#` wildcards:

- subscribe to `ecv1/*/config/cmd/get-configuration`
- subscribe to `ecv1/*/config/cmd/update-catalog`
- publish replies to `edgecommons/reply-*`
- publish pushes to `ecv1/*/*/cmd/set-config`
- publish its own `state` and `cfg`

Publishing to `update-catalog` is administrative and non-production. Ordinary consumer components
should have only the client-side permissions needed to request `get-configuration` and receive their
own `set-config` pushes. Production deployments should keep `allowVolatileCatalogUpdates` false and
deny publish access to the update topic where the broker/IPC policy allows it.

## Build And Packaging

Local development uses the default `standalone` feature and the sibling `../core/libs/rust` path
dependency:

```bash
cargo test
cargo build
```

Greengrass packaging uses `build.sh`, which builds the binary with `--no-default-features --features
greengrass` and stages the artifact expected by `recipe.yaml`.
