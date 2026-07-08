# ConfigComponent

`com.mbreissi.edgecommons.ConfigComponent` is the dedicated server for
`CONFIG_COMPONENT` hierarchical-config deployments. It loads a hierarchy catalog, serves ordered
lineage bundles to EdgeCommons components, and pushes accepted config updates to consumers over the
configured EdgeCommons messaging transport.

Use ConfigComponent when multiple components should inherit configuration from shared hierarchy
levels such as enterprise, site, building, zone, or line while keeping each component's own
`component` section separate. The catalog can contain any user-defined number of hierarchy levels;
the runtime device is the deepest level and is resolved by the running platform, not repeated as a
catalog node.

## Runtime Shape

The ConfigComponent starts from its own non-`CONFIG_COMPONENT` source:

- `GG_CONFIG` on Greengrass
- `FILE` or `ENV` on standalone hosts
- `CONFIGMAP` in Kubernetes

That bootstrap config points to a catalog source. The catalog contains `hierarchy.levels`, `nodes`,
and a `components` map keyed by sanitized component lookup token. Consumer components select
`-c CONFIG_COMPONENT`, request their lineage bundle from ConfigComponent, merge `layers[].config`
locally, validate the merged effective config, and hot-reload when the server pushes a replacement
bundle to their `set-config` inbox.

Catalog updates through the message interface are volatile and intended for debug, verification, and
test environments. They are disabled by default, never write back to the file or ConfigMap source,
and do not survive a restart.

See the [ConfigComponent reference](reference/config-component.md) for the bootstrap schema,
catalog format, request/reply topics, update behavior, and Greengrass IPC permissions.
