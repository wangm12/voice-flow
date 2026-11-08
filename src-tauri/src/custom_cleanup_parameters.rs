//! Bounded, validated top-level request parameters for the custom OpenAI-compatible
//! cleanup route. Values are sent only as request-body fields; credentials and
//! protocol/output-shape controls remain owned by VoiceFlow.

use serde_json::Value;
use std::collections::BTreeMap;

pub type Parameters = BTreeMap<String, Value>;

pub const MAX_PARAMETER_BYTES: usize = 8 * 1024;
const MAX_DEPTH: usize = 6;
const MAX_NODES: usize = 256;
const MAX_OBJECT_FIELDS: usize = 64;
const MAX_ARRAY_ITEMS: usize = 32;
const MAX_KEY_BYTES: usize = 64;
const MAX_STRING_BYTES: usize = 2 * 1024;

const RESERVED_TOP_LEVEL_FIELDS: &[&str] = &[
    "model",
    "messages",
    "prompt",
    "input",
    "file",
    "audio",
    "stream",
    "stream_options",
    "headers",
    "header",
    "auth",
    "authorization",
    "api_key",
    "apikey",
    "key",
    "tools",
    "tool_choice",
    "functions",
    "function_call",
    "response_format",
    "modalities",
    "output_format",
    "role",
    "content",
    "n",
    "parallel_tool_calls",
];

pub fn validate(parameters: &Parameters) -> Result<(), String> {
    let value = serde_json::to_value(parameters)
        .map_err(|_| "custom cleanup parameters must be a JSON object".to_owned())?;
    validate_value(&value)
}

pub fn validate_value(value: &Value) -> Result<(), String> {
    let Some(object) = value.as_object() else {
        return Err("custom cleanup parameters must be a JSON object".into());
    };
    let encoded = serde_json::to_vec(value)
        .map_err(|_| "custom cleanup parameters are not valid JSON".to_owned())?;
    if encoded.len() > MAX_PARAMETER_BYTES {
        return Err("custom cleanup parameters exceed the 8 KiB limit".into());
    }

    let mut nodes = 0usize;
    let mut normalized_names = std::collections::BTreeSet::new();
    for (key, value) in object {
        let normalized = normalize_key(key)?;
        if is_known_parameter(&normalized) && normalized != *key {
            return Err("known cleanup parameters must use their canonical field names".into());
        }
        if !normalized_names.insert(normalized.clone()) {
            return Err("custom cleanup parameter names must be unique".into());
        }
        if RESERVED_TOP_LEVEL_FIELDS.contains(&normalized.as_str()) {
            return Err(
                "custom cleanup parameters cannot replace protocol or output fields".into(),
            );
        }
        validate_tree_key(&normalized)?;
        validate_known_parameter(&normalized, value)?;
        validate_tree(value, 1, &mut nodes)?;
    }
    if normalized_names.contains("max_tokens") && normalized_names.contains("max_completion_tokens")
    {
        return Err("use either max_tokens or max_completion_tokens, not both".into());
    }
    Ok(())
}

fn normalize_key(key: &str) -> Result<String, String> {
    if key.is_empty()
        || key.len() > MAX_KEY_BYTES
        || key.chars().any(char::is_control)
        || key.trim() != key
    {
        return Err("custom cleanup parameter names must be short, non-empty field names".into());
    }
    Ok(key.to_ascii_lowercase().replace('-', "_"))
}

fn validate_tree_key(key: &str) -> Result<(), String> {
    if matches!(key, "__proto__" | "constructor" | "prototype")
        || key.contains("api_key")
        || key.contains("apikey")
        || key.contains("authorization")
        || key.contains("credential")
        || key.contains("password")
        || key.contains("secret")
        || key == "auth"
        || key.starts_with("auth_")
        || key == "bearer"
        || key == "token"
        || key.ends_with("_token")
    {
        return Err(
            "credential and authentication fields are not allowed in custom parameters".into(),
        );
    }
    Ok(())
}

fn validate_known_parameter(key: &str, value: &Value) -> Result<(), String> {
    match key {
        "temperature" => number_in_range(value, 0.0, 2.0, key),
        "top_p" | "min_p" => number_in_range(value, 0.0, 1.0, key),
        "presence_penalty" | "frequency_penalty" => number_in_range(value, -2.0, 2.0, key),
        "repetition_penalty" => number_in_range(value, 0.0, 2.0, key),
        "max_tokens" | "max_completion_tokens" => integer_in_range(value, 1, 131_072, key),
        "seed" => integer_in_range(value, 0, u32::MAX as i64, key),
        "top_k" => integer_in_range(value, 0, 1_000, key),
        "top_logprobs" => integer_in_range(value, 0, 20, key),
        "reasoning_effort" => {
            let allowed = [
                "default", "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
            ];
            if value.as_str().is_some_and(|candidate| {
                candidate.chars().count() <= 16 && allowed.contains(&candidate)
            }) {
                Ok(())
            } else {
                Err("reasoning_effort must be a supported bounded effort name".into())
            }
        }
        "verbosity" => {
            let allowed = ["low", "medium", "high"];
            if value
                .as_str()
                .is_some_and(|candidate| allowed.contains(&candidate))
            {
                Ok(())
            } else {
                Err("verbosity must be low, medium, or high".into())
            }
        }
        "logprobs" => {
            if value.is_boolean() {
                Ok(())
            } else {
                Err("logprobs must be a boolean".into())
            }
        }
        "include_reasoning" => {
            if value.is_boolean() {
                Ok(())
            } else {
                Err("include_reasoning must be a boolean".into())
            }
        }
        "stop" => {
            let valid_string = |item: &Value| {
                item.as_str()
                    .is_some_and(|text| !text.is_empty() && text.chars().count() <= 256)
            };
            if valid_string(value)
                || value.as_array().is_some_and(|items| {
                    (1..=4).contains(&items.len()) && items.iter().all(valid_string)
                })
            {
                Ok(())
            } else {
                Err("stop must be a short string or an array of up to four short strings".into())
            }
        }
        _ => Ok(()),
    }
}

fn is_known_parameter(key: &str) -> bool {
    matches!(
        key,
        "temperature"
            | "top_p"
            | "min_p"
            | "presence_penalty"
            | "frequency_penalty"
            | "repetition_penalty"
            | "max_tokens"
            | "max_completion_tokens"
            | "seed"
            | "top_k"
            | "top_logprobs"
            | "reasoning_effort"
            | "verbosity"
            | "logprobs"
            | "include_reasoning"
            | "stop"
    )
}

fn number_in_range(value: &Value, minimum: f64, maximum: f64, key: &str) -> Result<(), String> {
    if value
        .as_f64()
        .is_some_and(|number| number.is_finite() && (minimum..=maximum).contains(&number))
    {
        Ok(())
    } else {
        Err(format!("{key} is outside its supported numeric range"))
    }
}

fn integer_in_range(value: &Value, minimum: i64, maximum: i64, key: &str) -> Result<(), String> {
    if value
        .as_i64()
        .is_some_and(|number| (minimum..=maximum).contains(&number))
    {
        Ok(())
    } else {
        Err(format!(
            "{key} must be an integer within its supported range"
        ))
    }
}

fn validate_tree(value: &Value, depth: usize, nodes: &mut usize) -> Result<(), String> {
    *nodes = nodes.saturating_add(1);
    if *nodes > MAX_NODES || depth > MAX_DEPTH {
        return Err("custom cleanup parameters contain too many or too-deep values".into());
    }
    match value {
        Value::Object(object) => {
            if object.len() > MAX_OBJECT_FIELDS {
                return Err("custom cleanup parameter objects contain too many fields".into());
            }
            for (key, child) in object {
                let normalized = normalize_key(key)?;
                validate_tree_key(&normalized)?;
                validate_tree(child, depth + 1, nodes)?;
            }
        }
        Value::Array(items) => {
            if items.len() > MAX_ARRAY_ITEMS {
                return Err("custom cleanup parameter arrays contain too many items".into());
            }
            for child in items {
                validate_tree(child, depth + 1, nodes)?;
            }
        }
        Value::String(text) if text.len() > MAX_STRING_BYTES => {
            return Err("custom cleanup parameter strings exceed the 2 KiB limit".into());
        }
        Value::Number(number) if number.as_f64().is_none_or(|number| !number.is_finite()) => {
            return Err("custom cleanup parameters must contain finite JSON numbers".into());
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_empty_and_bounded_extension_values_without_losing_false_or_zero() {
        assert!(validate(&Parameters::new()).is_ok());
        let parameters = serde_json::from_value::<Parameters>(json!({
            "temperature": 0,
            "logprobs": false,
            "vendor_extension": {"enabled": false, "bias": 0, "labels": ["a", "b"]}
        }))
        .expect("map");
        assert!(validate(&parameters).is_ok());
        assert_eq!(parameters["temperature"], json!(0));
        assert_eq!(parameters["logprobs"], json!(false));
        assert_eq!(parameters["vendor_extension"]["bias"], json!(0));
    }

    #[test]
    fn rejects_protocol_credentials_and_output_shape_fields_at_any_depth() {
        for value in [
            json!({"model": "other-model"}),
            json!({"messages": []}),
            json!({"stream": false}),
            json!({"response_format": {"type": "json_object"}}),
            json!({"vendor": {"authorization": "secret"}}),
            json!({"vendor": {"apiKey": "secret"}}),
        ] {
            assert!(validate_value(&value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn validates_known_sampling_ranges_and_token_conflicts() {
        assert!(
            validate_value(&json!({"temperature": 0.7, "max_completion_tokens": 2048})).is_ok()
        );
        assert!(validate_value(&json!({"reasoning_effort": "default"})).is_ok());
        assert!(validate_value(&json!({"temperature": 2.1})).is_err());
        assert!(validate_value(&json!({"top_p": -0.1})).is_err());
        assert!(validate_value(&json!({"max_tokens": 200000})).is_err());
        assert!(validate_value(&json!({"max_tokens": 100, "max_completion_tokens": 100})).is_err());
    }

    #[test]
    fn known_string_limits_count_unicode_scalar_values() {
        let stop_at_100 = "停".repeat(100);
        assert!(validate_value(&json!({"stop": stop_at_100})).is_ok());
        let stop_at_limit = "停".repeat(256);
        let stop_over_limit = "停".repeat(257);
        assert!(validate_value(&json!({"stop": stop_at_limit})).is_ok());
        assert!(validate_value(&json!({"stop": stop_over_limit})).is_err());
        assert!(validate_value(&json!({"repetition_penalty": 0})).is_ok());
        assert!(validate_value(&json!({"repetition_penalty": -0.01})).is_err());
    }

    #[test]
    fn rejects_oversized_and_deep_extension_payloads() {
        let oversized = "x".repeat(MAX_PARAMETER_BYTES + 1);
        assert!(validate_value(&json!({"vendor": oversized})).is_err());
        assert!(
            validate_value(&json!({"vendor": {"a": {"b": {"c": {"d": {"e": {"f": 1}}}}}}}))
                .is_err()
        );
    }
}
