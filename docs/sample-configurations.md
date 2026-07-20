# Sample configurations

Worked configurations for the ConfigComponent, starting from the runnable set the repo ships under
`test-configs/`. For every field's meaning see [reference/configuration.md](reference/configuration.md).

## The shipped `test-configs/`

- `config.json` — the bootstrap config. Its `catalogSource` is a `file` source pointing at
  `test-configs/catalog.json` with `watch: true`, `pushOnCatalogReload: true`, and
  `allowVolatileCatalogUpdates: false`. It also carries the standard `logging`, `heartbeat`, and
  `metricEmission` envelope blocks.
- `catalog.json` — a five-level `enterprise → site → zone → line → device` hierarchy with four scope
  nodes and two components (`opcua-adapter`, `modbus-adapter`) parented at `line/line-7`.
- `standalone-messaging.json` — the HOST/MQTT messaging config (a local broker on `localhost:1883`).

Run the three together:

```bash
cargo run -- --platform HOST --transport MQTT test-configs/standalone-messaging.json \
  -c FILE test-configs/config.json -t gw-01
```

## Bootstrap: file source with hot reload

The shipped bootstrap shape, annotated:

```json
{
  "component": {
    "token": "edgecommons-config-component",
    "global": {
      "configComponent": {
        "catalogSource": { "type": "file", "path": "test-configs/catalog.json", "watch": true },
        "pushOnCatalogReload": true,
        "allowVolatileCatalogUpdates": false
      }
    },
    "instances": []
  }
}
```

## Bootstrap: Kubernetes ConfigMap source

For the Kubernetes deployment (`k8s/configmap.yaml`), the catalog source reads a sibling ConfigMap key,
mounted read/watch only at `/etc/edgecommons`:

```json
{
  "component": {
    "token": "edgecommons-config-component",
    "global": {
      "configComponent": {
        "catalogSource": { "type": "configmap", "mountDir": "/etc/edgecommons", "key": "catalog.json", "watch": true },
        "pushOnCatalogReload": true,
        "allowVolatileCatalogUpdates": false
      }
    },
    "instances": []
  }
}
```

## Bootstrap: environment-variable source with volatile updates (test only)

A non-production variant that loads a snapshot catalog from an environment variable and enables the
`update-catalog` message interface for verification:

```json
{
  "component": {
    "token": "edgecommons-config-component",
    "global": {
      "configComponent": {
        "catalogSource": { "type": "env", "var": "EDGECOMMONS_CONFIG_CATALOG" },
        "pushOnCatalogReload": true,
        "allowVolatileCatalogUpdates": true
      }
    },
    "instances": []
  }
}
```

Environment sources are snapshot-only (no `watch`); combine this with volatile `update-catalog` sends to
drive catalog changes during a test.

## A non-trivial catalog: multiple lines, shared scopes

A catalog where two lines under the same site share the enterprise and site scopes but diverge below,
and two components inherit from different lines:

```json
{
  "schemaVersion": 1,
  "version": "dallas-2026-07-19",
  "hierarchy": { "levels": ["enterprise", "site", "zone", "line", "device"] },
  "nodes": {
    "enterprise/acme": {
      "scope": { "enterprise": "acme" },
      "config": { "identity": { "enterprise": "acme" }, "logging": { "level": "INFO" } }
    },
    "site/dallas": {
      "parent": "enterprise/acme",
      "scope": { "enterprise": "acme", "site": "dallas" },
      "config": { "identity": { "site": "dallas" } }
    },
    "line/line-7": {
      "parent": "site/dallas",
      "scope": { "enterprise": "acme", "site": "dallas", "line": "line-7" },
      "config": { "identity": { "line": "line-7" }, "component": { "global": { "pollIntervalMs": 1000 } } }
    },
    "line/line-9": {
      "parent": "site/dallas",
      "scope": { "enterprise": "acme", "site": "dallas", "line": "line-9" },
      "config": { "identity": { "line": "line-9" }, "component": { "global": { "pollIntervalMs": 500 } } }
    }
  },
  "components": {
    "opcua-adapter": {
      "parent": "line/line-7",
      "config": { "component": { "token": "opcua-adapter", "global": { "endpoint": "opc.tcp://plc-7:4840" } } }
    },
    "modbus-adapter": {
      "parent": "line/line-9",
      "config": { "component": { "token": "modbus-adapter", "global": { "unitId": 3 } } }
    }
  }
}
```

`opcua-adapter` inherits `enterprise/acme → site/dallas → line/line-7`; `modbus-adapter` inherits
`enterprise/acme → site/dallas → line/line-9`. Each component's `get-configuration` reply carries only
its own lineage in order, and the client merges from the top scope down to the component layer.
