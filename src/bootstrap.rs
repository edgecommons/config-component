//! Pure bootstrap/config helpers, kept out of the live messaging seam (`server.rs`) so they stay
//! unit-tested and inside the coverage denominator.

use std::ffi::OsString;

use edgecommons::messaging::Message;
use serde_json::Value;

/// Fail fast if this component tries to bootstrap from itself.
///
/// `com.mbreissi.edgecommons.ConfigComponent` serves the `CONFIG_COMPONENT` rendezvous, so it must
/// never resolve its own config through `CONFIG_COMPONENT`. Any `-c CONFIG_COMPONENT` / `--config
/// CONFIG_COMPONENT` / `-cCONFIG_COMPONENT` form is rejected before subscriptions are created.
pub fn reject_recursive_bootstrap<I, T>(args: I) -> anyhow::Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let values = args
        .into_iter()
        .map(|arg| arg.into().to_string_lossy().to_string())
        .collect::<Vec<_>>();

    for (index, arg) in values.iter().enumerate() {
        let token = if matches!(arg.as_str(), "-c" | "--config") {
            values.get(index + 1).map(String::as_str)
        } else if let Some(value) = arg.strip_prefix("--config=") {
            Some(value)
        } else {
            arg.strip_prefix("-c").filter(|value| !value.is_empty())
        };

        if token.is_some_and(|value| value.eq_ignore_ascii_case("CONFIG_COMPONENT")) {
            anyhow::bail!(
                "com.mbreissi.edgecommons.ConfigComponent cannot bootstrap from CONFIG_COMPONENT; use GG_CONFIG, FILE, ENV, or CONFIGMAP"
            );
        }
    }

    Ok(())
}

/// Read an optional boolean field from a config object, defaulting when absent and erroring when the
/// value is present but not a boolean.
pub(crate) fn optional_bool(object: &Value, field: &str, default: bool) -> anyhow::Result<bool> {
    object.get(field).map_or(Ok(default), |value| {
        value.as_bool().ok_or_else(|| {
            anyhow::anyhow!("component.global.configComponent.{field} must be a boolean")
        })
    })
}

/// Extract the request body: the raw JSON body when one is present, otherwise the structured body.
pub(crate) fn message_body(message: &Message) -> Value {
    message
        .get_raw()
        .cloned()
        .unwrap_or_else(|| message.body.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgecommons::messaging::MessageBuilder;
    use serde_json::json;

    #[test]
    fn rejects_config_component_bootstrap_forms() {
        assert!(reject_recursive_bootstrap(["config-component", "-c", "CONFIG_COMPONENT"]).is_err());
        assert!(
            reject_recursive_bootstrap(["config-component", "--config=CONFIG_COMPONENT"]).is_err()
        );
        assert!(reject_recursive_bootstrap(["config-component", "-cCONFIG_COMPONENT"]).is_err());
        // Case-insensitive.
        assert!(reject_recursive_bootstrap(["config-component", "-c", "config_component"]).is_err());
    }

    #[test]
    fn allows_non_recursive_bootstrap_sources() {
        reject_recursive_bootstrap(["config-component", "-c", "FILE", "config.json"]).unwrap();
        reject_recursive_bootstrap(["config-component", "--platform", "GREENGRASS"]).unwrap();
        // A trailing `-c` with no value is not a CONFIG_COMPONENT selection.
        reject_recursive_bootstrap(["config-component", "-c"]).unwrap();
    }

    #[test]
    fn optional_bool_defaults_reads_and_rejects() {
        let object = json!({ "yes": true, "no": false, "notBool": 7 });
        assert!(optional_bool(&object, "yes", false).unwrap());
        assert!(!optional_bool(&object, "no", true).unwrap());
        // Absent -> default.
        assert!(optional_bool(&object, "missing", true).unwrap());
        assert!(!optional_bool(&object, "missing", false).unwrap());
        // Present but not a boolean -> error.
        assert!(optional_bool(&object, "notBool", false).is_err());
    }

    #[test]
    fn message_body_prefers_raw_then_structured() {
        // A message built with a raw JSON payload returns that raw body.
        let raw = MessageBuilder::new("GetConfiguration", "1.0")
            .payload(json!({ "component": "opcua-adapter" }))
            .build();
        assert_eq!(message_body(&raw)["component"], "opcua-adapter");
    }
}
