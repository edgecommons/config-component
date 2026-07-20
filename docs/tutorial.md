# Tutorial — Serve a hierarchical config and watch a hot reload

By the end you will have run the ConfigComponent against a local catalog, requested a component's
lineage bundle over MQTT, and watched the server hot-promote and push a new catalog when the file
changes.

## 1. Prerequisites

- A Rust toolchain (edition 2021, `rust-version = "1.85"`).
- A local MQTT broker on `localhost:1883` (`docker run -d -p 1883:1883 emqx/emqx`).
- `mosquitto_pub` / `mosquitto_sub` (or any MQTT client).

## 2. Build it

```bash
cargo build
```

## 3. Run it

The repo ships a runnable bootstrap config and a sample catalog under `test-configs/`. The bootstrap
config points its `catalogSource` at `test-configs/catalog.json` with `watch: true`.

```bash
cargo run -- \
  --platform HOST \
  --transport MQTT test-configs/standalone-messaging.json \
  -c FILE test-configs/config.json \
  -t gw-01
```

The server subscribes to the two rendezvous topics for device `gw-01`:

```text
ecv1/gw-01/config/cmd/get-configuration
ecv1/gw-01/config/cmd/update-catalog
```

## 4. Request a lineage bundle

Subscribe to a reply topic, then ask for the `opcua-adapter` component's configuration. With a
request/reply MQTT client the reply comes back on the `reply_to` topic; the simplest illustration is to
watch the server's pushes instead (next step). To drive the request directly:

```bash
mosquitto_pub -t 'ecv1/gw-01/config/cmd/get-configuration' \
  -m '{"header":{"name":"get-configuration","version":"1.0","replyTo":"demo/reply"},"body":{"component":"opcua-adapter"}}'
mosquitto_sub -t 'demo/reply' -v
```

The reply is an ordered `layers[]` bundle from `enterprise/acme` down to `component/opcua-adapter`. The
client merges `layers[].config` to get the component's effective config.

## 5. Watch a hot reload

Subscribe to every component's `set-config` inbox:

```bash
mosquitto_sub -t 'ecv1/gw-01/+/cmd/set-config' -v
```

Now edit `test-configs/catalog.json` — bump `"version"` and change a value under one component's
`config`. Within a second the watching server notices the content change, promotes the new catalog, and
(because `pushOnCatalogReload` is `true`) publishes a fresh lineage bundle to every component. You see a
`set-config` message land for each component in the catalog.

## 6. Prove it end-to-end

```bash
cargo test
```

The suite exercises catalog parsing and lineage building, the file/ConfigMap/env sources, and the
coordinator's promote / serve / update / reject-and-keep logic — no broker required.

Next: the [how-to guides](how-to-guides.md) for choosing a catalog source and deploying; the
[reference](reference/configuration.md) for every option and topic; the [explanation](explanation.md)
for why the server serves unmerged layers.
