# Reference — Metrics

The ConfigComponent emits no custom application metrics. As a small always-on service, its
observability comes entirely from the standard EdgeCommons library envelope, configured with the same
`logging`, `heartbeat`, and `metricEmission` blocks as any other component.

## State keepalive (`state` class, reserved)

The library publishes a periodic keepalive on the reserved `state` class:

```text
ecv1/{device}/edgecommons-config-component/state
```

A whole-fleet consumer covers it with the `ecv1/+/+/+/state` wildcard. The keepalive reflects the
process's RUNNING status and its heartbeat measures.

## Heartbeat

With the shipped configuration, `heartbeat` is enabled with a `local` destination and CPU/memory
measures:

```json
{
  "heartbeat": {
    "enabled": true,
    "intervalSecs": 5,
    "destination": "local",
    "measures": { "cpu": true, "memory": true, "disk": false }
  }
}
```

## Metric emission target

`metricEmission.target` selects where the library's own metrics go. The shipped configuration uses the
`log` target (a local rotating file); `messaging` (the reserved UNS `metric` class), `cloudwatch`, and
`prometheus` targets are also available — see the core library's platform documentation.

```json
{
  "metricEmission": {
    "target": "log",
    "targetConfig": { "logFileName": "/greengrass/v2/logs/{ComponentFullName}.metric.log" }
  }
}
```
