# ConfigComponent Reference

`com.mbreissi.edgecommons.ConfigComponent` is the built-in server role for
`CONFIG_COMPONENT` split-config deployments. It is implemented in Rust. Ordinary EdgeCommons
components remain clients.

## Bootstrap

The component must start from `GG_CONFIG`, `FILE`, `ENV`, or `CONFIGMAP`. Launching it with
`-c CONFIG_COMPONENT` is rejected before subscriptions are created. On Greengrass, the recipe uses
the platform default `GG_CONFIG` and reads `ComponentConfig`.

Required bootstrap config:

- `component.token`: should be `edgecommons-config-component`
- `component.global.configComponent.catalogSource`: v1 `file` or `configmap` source descriptor
- `component.global.configComponent.pushOnCatalogReload`: optional boolean, default `true`
- `component.global.configComponent.allowVolatileCatalogUpdates`: optional boolean, default `false`

The bootstrap config is the ConfigComponent's own effective config. It is not a catalog entry and is
not returned to other components.

## Catalog Source

Catalog loading goes through the `CatalogSource` seam. v1 implements local JSON file and
Kubernetes ConfigMap-mounted file sources:

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

`file.path` is required and non-empty. `configmap` may use either `path`, or `mountDir` plus `key`;
`key` must be a file name inside the mount. `watch` is optional (`false` by default). If `version` is
absent in a source-loaded catalog, the server derives `version` from a `sha256:` content hash. If
`provenance` is absent, it derives `source`, `uri`, and `contentHash` from the source descriptor.

ConfigMap sources are read/watch only. A ConfigMap change is applied by Kubernetes, observed through
the mounted file, and promoted by the ConfigComponent. The component does not write back to the
ConfigMap. Future sources, such as git-backed catalogs, plug in behind the same load/watch contract.

## Catalog Format

```json
{
  "schemaVersion": 1,
  "version": "2026-07-07T18:30:00Z",
  "provenance": { "source": "file", "uri": "/etc/edgecommons/catalog.json" },
  "base": { "logging": { "level": "INFO" } },
  "components": {
    "opcua-adapter": { "component": { "token": "opcua-adapter" } }
  }
}
```

`base` may be absent or `null`. `components` is keyed by sanitized short component token. Component
entries are raw component layers; the server never schema-validates them as effective configs.
`schemaVersion` must be `1`. A message-delivered catalog must include a non-empty `version`; file
catalogs may derive one from content.

## Request And Update Topics

- GET: `ecv1/{device}/config/main/cmd/get-configuration`
- update: `ecv1/{device}/config/main/cmd/update-catalog`
- push: `ecv1/{device}/{component}/main/cmd/set-config`

All messages use the normal EdgeCommons message envelope. The server reads a raw JSON body when one
is present, otherwise it reads the structured message body. If a request has `reply_to`, the server
replies; otherwise it processes the request without an acknowledgement.

GET requires body:

```json
{ "component": "opcua-adapter" }
```

Successful GET replies are raw layer bundles:

```json
{
  "base": { "logging": { "level": "INFO" } },
  "component": { "component": { "token": "opcua-adapter" } }
}
```

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
    "components": {
      "opcua-adapter": { "component": { "token": "opcua-adapter" } }
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
replies with the promoted version/provenance, and publishes `SetConfig` bundles to every component
in the catalog when `pushOnCatalogReload` is `true`. It never writes the replacement to the active
file, ConfigMap, or future catalog source. The override is gone after restart or after a later
source-side reload replaces it. Disabled updates, invalid updates, and invalid source reloads keep
the previous active catalog and do not push.

## Error Codes

- `BAD_REQUEST`: request body is not an object or lacks a required string/object field.
- `CONFIG_NOT_FOUND`: the requested component token is absent from the active catalog.
- `CATALOG_UNAVAILABLE`: no valid catalog is currently loaded.
- `CATALOG_INVALID`: catalog shape, schema version, component keys, version matching, or layer shape is invalid.
- `CATALOG_UPDATE_DISABLED`: volatile message updates are disabled.

Failures keep the previous active catalog and do not push `set-config`.

## Greengrass Permissions

The recipe grants the server only the protocol topics it needs. These are Greengrass IPC authorization
resource patterns, so they use Greengrass `*` matching instead of MQTT `+` or `#` wildcards:

- subscribe to `ecv1/*/config/main/cmd/get-configuration`
- subscribe to `ecv1/*/config/main/cmd/update-catalog`
- publish replies to `edgecommons/reply-*`
- publish pushes to `ecv1/*/*/main/cmd/set-config`
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
