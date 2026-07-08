# ConfigComponent

`com.mbreissi.edgecommons.ConfigComponent` is the dedicated server for
`CONFIG_COMPONENT` split-config deployments. It loads a shared catalog, serves raw base and
component-layer bundles to EdgeCommons components, and pushes accepted config updates to consumers
over the configured EdgeCommons messaging transport.

Use ConfigComponent when multiple components on the same device should share framework sections
such as logging, credentials, parameters, tags, and stream definitions while keeping each
component's own `component` section separate.

## Runtime Shape

The ConfigComponent starts from its own non-`CONFIG_COMPONENT` source:

- `GG_CONFIG` on Greengrass
- `FILE` or `ENV` on standalone hosts
- `CONFIGMAP` in Kubernetes

That bootstrap config points to a catalog source. The catalog contains an optional shared `base`
layer and a `components` map keyed by component token. Consumer components select
`-c CONFIG_COMPONENT`, request their layer bundle from ConfigComponent, merge the layers locally,
and hot-reload when the server pushes a replacement bundle to their `set-config` inbox.

Catalog updates through the message interface are volatile and intended for debug, verification, and
test environments. They are disabled by default, never write back to the file or ConfigMap source,
and do not survive a restart.

See the [ConfigComponent reference](reference/config-component.md) for the bootstrap schema,
catalog format, request/reply topics, update behavior, and Greengrass IPC permissions.
