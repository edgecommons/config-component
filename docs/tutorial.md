# Tutorial — Serve a hierarchical config and watch a hot reload

By the end you will have run the ConfigComponent against a local catalog, requested a component's
lineage bundle over MQTT, and watched the server hot-promote and push a new catalog when the file
changes.

## 1. Prerequisites

- A current stable Rust toolchain for the locked dependency graph (the crate declares a 1.85 MSRV).
- A local MQTT broker on `localhost:1883` (`docker run -d -p 1883:1883 emqx/emqx`).
- Python with `paho-mqtt` and the matching EdgeCommons decoder. In the organization workspace:
  `pip install paho-mqtt -e ../core/libs/python`.
- The Python here-documents use a Bash-compatible shell; in PowerShell, save the Python contents
  to a `.py` file and run `python <file>.py`.

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

This rendezvous service returns a **bare lineage bundle body** in an EdgeCommons protobuf reply.
It does not return the CommandsRegistry `{ok, result|error}` wrapper that `ec-uns-cmd` expects.
Use this bounded client: it subscribes before publishing, supplies a correlation id, decodes protobuf,
and prints the reply body as JSON. The request body is a native JSON object passed to the builder.

```bash
python - <<'PY'
import json
import threading
import uuid
import paho.mqtt.client as mqtt
from edgecommons.messaging.identity import HierEntry, MessageIdentity
from edgecommons.messaging.message import Message
from edgecommons.messaging.message_builder import MessageBuilder

correlation = str(uuid.uuid4())
reply_topic = "edgecommons/reply-" + str(uuid.uuid4())
subscribed, received = threading.Event(), threading.Event()
replies = []
c = mqtt.Client(mqtt.CallbackAPIVersion.VERSION2)
c.on_connect = lambda c, u, f, rc, p: c.subscribe(reply_topic, qos=1)
c.on_subscribe = lambda c, u, mid, reasons, p: subscribed.set()
def on_message(c, u, m):
    message = Message.from_bytes(m.payload)
    if message.header.correlation_id == correlation:
        replies.append(message.get_body())
        received.set()
c.on_message = on_message
c.connect("localhost", 1883)
c.loop_start()
try:
    if not subscribed.wait(5):
        raise TimeoutError("reply subscription was not acknowledged")
    request = (MessageBuilder.create("get-configuration", "1.0")
        .with_identity(MessageIdentity([HierEntry("device", "gw-01")], "config-demo"))
        .with_command({"component": "opcua-adapter"})
        .with_reply_to(reply_topic).with_correlation_id(correlation).build())
    c.publish("ecv1/gw-01/config/cmd/get-configuration", request.to_bytes(), qos=1).wait_for_publish(timeout=5)
    if not received.wait(10):
        raise TimeoutError("no correlated lineage reply within 10 seconds")
    print(json.dumps(replies[0], indent=2))
finally:
    c.unsubscribe(reply_topic)
    c.disconnect()
    c.loop_stop()
PY
```

The successful reply has ordered `layers[]` from `enterprise/acme` down to
`component/opcua-adapter`. The client merges `layers[].config` and validates the effective config.

## 5. Watch a hot reload

Decode the component-scope `set-config` pushes as a human-readable JSON projection:

```bash
python - <<'PY'
import json
import paho.mqtt.client as mqtt
from edgecommons.messaging.message import Message

c = mqtt.Client(mqtt.CallbackAPIVersion.VERSION2)
c.on_connect = lambda c, u, f, rc, p: c.subscribe("ecv1/gw-01/+/cmd/set-config", qos=1)
def on_message(c, u, m):
    message = Message.from_bytes(m.payload)
    print(m.topic, json.dumps(message.to_diagnostic_json(), indent=2))
c.on_message = on_message
c.connect("localhost", 1883)
try:
    c.loop_forever()
except KeyboardInterrupt:
    pass
finally:
    c.unsubscribe("ecv1/gw-01/+/cmd/set-config")
    c.disconnect()
PY
```

Edit the native JSON file `test-configs/catalog.json`: bump `version` and change a value under a
component's `config`. With `watch: true` and `pushOnCatalogReload: true`, the server detects a valid
catalog change and pushes a fresh lineage bundle. Invalid replacements are rejected while the last
valid catalog remains active. Stop the watcher with Ctrl-C when finished.

## 6. Verify the catalog logic

```bash
cargo test
```

The suite exercises catalog parsing and lineage building, the file/ConfigMap/env sources, and the
coordinator's promote / serve / update / reject-and-keep logic — no broker required.

Next: the [how-to guides](how-to-guides.md) for choosing a catalog source and deploying; the
[reference](reference/configuration.md) for every option and topic; the [explanation](explanation.md)
for why the server serves unmerged layers.
