# Reference — Messaging Interface & CLI

Every topic the ConfigComponent serves or publishes, the request/reply bodies, the error codes, the CLI
flags, and the Greengrass IPC permissions. Addressing follows the **Unified Namespace (UNS)**:
`ecv1/{device}/{component}[/{instance}]/{class}[/channel]`.

## Topics

| Class | Message | Direction | Topic | Reply |
|-------|---------|-----------|-------|-------|
| `cmd` | `get-configuration` | bus → server | `ecv1/{device}/config/cmd/get-configuration` | lineage bundle or error |
| `cmd` | `update-catalog` | bus → server | `ecv1/{device}/config/cmd/update-catalog` | `CatalogUpdateAck` or error |
| `cmd` | `set-config` | server → component | `ecv1/{device}/{component}/cmd/set-config` | — |

`{device}` is the resolved Thing name. Normal MQTT and Greengrass IPC requests carry protobuf bytes;
the server reads the decoded message body. The JSON objects below are native request/reply body
shapes, not complete wire envelopes. If a request carries `header.reply_to`, the server replies; otherwise it processes the
request without an acknowledgement.

## `get-configuration`

Request body:

```json
{ "component": "opcua-adapter" }
```

A successful reply is a lineage bundle — an ordered `layers[]` array from the highest shared scope down
to the requested component:

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

A valid client merges only `layers[].config`, in order, and validates the merged effective config. It
rejects legacy component-only documents and old `{base, component}` bundles.

## `update-catalog`

`update-catalog` is a non-production debug/verification/test interface. It is disabled by default; enable
it only in non-production with `component.global.configComponent.allowVolatileCatalogUpdates: true`. An
update is a complete replacement; the top-level `version` must match `catalog.version`:

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
          "identity": { "enterprise": "acme", "site": "dallas", "zone": "packaging", "line": "line-7" },
          "component": { "token": "opcua-adapter" }
        }
      }
    }
  }
}
```

A successful reply is a `CatalogUpdateAck`:

```json
{
  "ok": true,
  "version": "git:8f2c9c7",
  "provenance": { "source": "message", "interface": "update-catalog", "volatile": true }
}
```

On a valid enabled update, the server promotes the replacement only to the active in-memory cache, replies
with the promoted version and provenance, and — when `pushOnCatalogReload` is `true` — publishes lineage
bundles to every component in the catalog. It never writes the replacement to the active file or ConfigMap
source, so the override is gone after a restart or after a later source-side reload replaces it. Disabled
updates, invalid updates, and invalid source reloads keep the previous active catalog and do not push.

## `set-config` push

When a new catalog is promoted (a source-side reload or an accepted volatile update) and
`pushOnCatalogReload` is `true`, the server publishes a complete lineage bundle — the same shape as a
`get-configuration` reply — to each component's own inbox:

```text
ecv1/{device}/{component}/cmd/set-config
```

## Error codes

| Code | Meaning |
|------|---------|
| `BAD_REQUEST` | The request body is not an object, or lacks a required string/object field. |
| `CONFIG_NOT_FOUND` | The requested component token is absent from the active catalog. |
| `CATALOG_UNAVAILABLE` | No valid catalog is currently loaded. |
| `CATALOG_INVALID` | Catalog shape, schema version, hierarchy, node ids, component keys, version matching, or layer shape is invalid. |
| `CATALOG_UPDATE_DISABLED` | Volatile message updates are disabled. |
| `LINEAGE_CYCLE` | A component or node lineage revisits a node. |
| `LINEAGE_PARENT_MISSING` | A node or component references a missing parent. |
| `LINEAGE_DEPTH_EXCEEDED` | A lineage exceeds the maximum supported depth. |
| `LINEAGE_SCOPE_CONFLICT` | A child scope overwrites an ancestor scope value. |
| `LINEAGE_IDENTITY_CONFLICT` | A layer identity overwrites an ancestor-owned identity value. |

An error reply is `{ "ok": false, "error": { "code": ..., "message": ... } }`. Failures keep the previous
active catalog and do not push `set-config`.

## CLI

| Flag | Values | Notes |
|------|--------|-------|
| `--platform` | `GREENGRASS` \| `HOST` \| `KUBERNETES` \| `auto` | Default `auto`. |
| `--transport` | `IPC` \| `MQTT [path]` | IPC is Greengrass-only; HOST/Kubernetes use MQTT. |
| `-c/--config` | `GG_CONFIG` \| `FILE <path>` \| `ENV` \| `CONFIGMAP` | The bootstrap source. `CONFIG_COMPONENT` is rejected. |
| `-t/--thing` | `<name>` | Thing name; the `{device}` token of the rendezvous topics. |

## Greengrass IPC permissions

The recipe grants the server only the protocol topics it needs. These are Greengrass IPC authorization
resource patterns, so they use Greengrass `*` matching instead of MQTT `+` or `#` wildcards:

- subscribe to `ecv1/*/config/cmd/get-configuration`
- subscribe to `ecv1/*/config/cmd/update-catalog`
- publish replies to `edgecommons/reply-*`
- publish pushes to `ecv1/*/*/cmd/set-config`
- publish its own `state` and `cfg`

Publishing to `update-catalog` is administrative and non-production. Ordinary consumer components should
have only the client-side permissions needed to request `get-configuration` and receive their own
`set-config` pushes. Production deployments keep `allowVolatileCatalogUpdates` false and deny publish
access to the update topic where the broker/IPC policy allows it.
