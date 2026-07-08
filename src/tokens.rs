//! Token normalization shared by request routing and push topic generation.

use edgecommons::config::template::sanitize;

/// Sanitize a value with the same blacklist used by EdgeCommons UNS topics.
pub fn sanitize_token(value: &str) -> String {
    sanitize(value)
}

/// Return the sanitized short component token used as a catalog key.
///
/// Full Greengrass component names are reduced to the segment after the last dot,
/// matching the `CONFIG_COMPONENT` client self-identification path in core.
pub fn short_component_token(component: &str) -> Result<String, &'static str> {
    let trimmed = component.trim();
    if trimmed.is_empty() {
        return Err("component must be non-empty");
    }
    let short = trimmed.rsplit('.').next().unwrap_or(trimmed);
    Ok(sanitize_token(short))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_component_token_uses_last_name_segment_and_sanitizes() {
        assert_eq!(
            short_component_token("com.example.My/Component").unwrap(),
            "My_Component"
        );
        assert_eq!(
            short_component_token("modbus-adapter").unwrap(),
            "modbus-adapter"
        );
    }
}
