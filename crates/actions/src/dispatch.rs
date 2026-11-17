use serde::Serialize;
use std::collections::BTreeMap;
use yaml_serde::Value;

const MAX_INPUTS: usize = 25;
const MAX_INPUT_NAME_BYTES: usize = 100;
const MAX_INPUT_VALUE_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InputKind {
    String,
    Boolean,
    Choice,
    Number,
}

#[derive(Clone, Debug, Serialize)]
pub struct DispatchInput {
    pub name: String,
    pub description: Option<String>,
    pub required: bool,
    #[serde(rename = "type")]
    pub kind: InputKind,
    pub default: Option<String>,
    pub options: Vec<String>,
}

fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn valid_input_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_INPUT_NAME_BYTES
        && !name.as_bytes()[0].is_ascii_digit()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// Parses `on.workflow_dispatch`; `None` and `null` declare no inputs.
pub fn parse_inputs(config: Option<&Value>) -> Result<Vec<DispatchInput>, String> {
    let Some(config) = config.filter(|config| !config.is_null()) else {
        return Ok(Vec::new());
    };
    let config = config
        .as_mapping()
        .ok_or_else(|| "workflow_dispatch configuration must be a mapping".to_owned())?;
    if config.keys().any(|key| key.as_str() != Some("inputs")) {
        return Err("workflow_dispatch supports only `inputs`".to_owned());
    }
    let Some(inputs) = config.get("inputs").filter(|inputs| !inputs.is_null()) else {
        return Ok(Vec::new());
    };
    let inputs = inputs
        .as_mapping()
        .ok_or_else(|| "workflow_dispatch inputs must be a mapping".to_owned())?;
    if inputs.len() > MAX_INPUTS {
        return Err(format!(
            "workflow_dispatch supports at most {MAX_INPUTS} inputs"
        ));
    }
    let mut parsed = Vec::with_capacity(inputs.len());
    for (name, definition) in inputs {
        let name = name
            .as_str()
            .filter(|name| valid_input_name(name))
            .ok_or_else(|| "workflow_dispatch input names must be identifiers".to_owned())?;
        let empty = yaml_serde::Mapping::new();
        let definition = match definition {
            Value::Null => &empty,
            Value::Mapping(definition) => definition,
            _ => return Err(format!("input `{name}` must be a mapping")),
        };
        for key in definition.keys() {
            if !matches!(
                key.as_str(),
                Some("description" | "required" | "type" | "default" | "options")
            ) {
                return Err(format!("input `{name}` has an unsupported key"));
            }
        }
        let description = match definition.get("description") {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) => Some(text.clone()),
            Some(_) => return Err(format!("input `{name}` description must be a string")),
        };
        let required = match definition.get("required") {
            None => false,
            Some(Value::Bool(value)) => *value,
            Some(_) => return Err(format!("input `{name}` required must be a boolean")),
        };
        let kind = match definition.get("type").map(Value::as_str) {
            None | Some(Some("string")) => InputKind::String,
            Some(Some("boolean")) => InputKind::Boolean,
            Some(Some("choice")) => InputKind::Choice,
            Some(Some("number")) => InputKind::Number,
            _ => {
                return Err(format!(
                    "input `{name}` type must be string, boolean, choice, or number"
                ));
            }
        };
        let options = match definition.get("options") {
            None => Vec::new(),
            Some(Value::Sequence(options)) => options
                .iter()
                .map(|option| {
                    scalar(option).ok_or_else(|| format!("input `{name}` options must be scalars"))
                })
                .collect::<Result<_, _>>()?,
            Some(_) => return Err(format!("input `{name}` options must be a list")),
        };
        if (kind == InputKind::Choice) == options.is_empty() {
            return Err(format!(
                "input `{name}` must declare options exactly when its type is choice"
            ));
        }
        let default = match definition.get("default") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                scalar(value).ok_or_else(|| format!("input `{name}` default must be a scalar"))?,
            ),
        };
        let input = DispatchInput {
            name: name.to_owned(),
            description,
            required,
            kind,
            default,
            options,
        };
        if let Some(default) = &input.default {
            check_value(&input, default)?;
        }
        parsed.push(input);
    }
    Ok(parsed)
}

fn check_value(input: &DispatchInput, value: &str) -> Result<(), String> {
    let name = &input.name;
    if value.len() > MAX_INPUT_VALUE_BYTES {
        return Err(format!(
            "input `{name}` exceeds {MAX_INPUT_VALUE_BYTES} bytes"
        ));
    }
    match input.kind {
        InputKind::String => Ok(()),
        InputKind::Boolean if matches!(value, "true" | "false") => Ok(()),
        InputKind::Boolean => Err(format!("input `{name}` must be true or false")),
        InputKind::Number if value.parse::<f64>().is_ok_and(f64::is_finite) => Ok(()),
        InputKind::Number => Err(format!("input `{name}` must be a number")),
        InputKind::Choice if input.options.iter().any(|option| option == value) => Ok(()),
        InputKind::Choice => Err(format!(
            "input `{name}` must be one of: {}",
            input.options.join(", ")
        )),
    }
}

/// Combines provided values with defaults. Values are strings, as in
/// `github.event.inputs`; the runner derives the typed `inputs` context.
pub fn resolve_inputs(
    declared: &[DispatchInput],
    provided: &BTreeMap<String, serde_json::Value>,
) -> Result<BTreeMap<String, String>, String> {
    if let Some(unknown) = provided
        .keys()
        .find(|name| !declared.iter().any(|input| &input.name == *name))
    {
        return Err(format!("the workflow does not declare input `{unknown}`"));
    }
    let mut resolved = BTreeMap::new();
    for input in declared {
        let value = match provided.get(&input.name) {
            Some(serde_json::Value::String(value)) => Some(value.clone()),
            Some(serde_json::Value::Bool(value)) => Some(value.to_string()),
            Some(serde_json::Value::Number(value)) => Some(value.to_string()),
            Some(serde_json::Value::Null) | None => None,
            Some(_) => return Err(format!("input `{}` must be a scalar", input.name)),
        };
        let value = match value.or_else(|| input.default.clone()) {
            Some(value) => value,
            None if input.required => {
                return Err(format!("input `{}` is required", input.name));
            }
            None if input.kind == InputKind::Boolean => "false".to_owned(),
            None => String::new(),
        };
        if !value.is_empty() || input.kind != InputKind::String {
            check_value(input, &value)?;
        }
        resolved.insert(input.name.clone(), value);
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn inputs(source: &str) -> Result<Vec<DispatchInput>, String> {
        let value: Value = yaml_serde::from_str(source).unwrap();
        parse_inputs(Some(&value))
    }

    #[test]
    fn inputs_are_parsed_and_validated() {
        let declared = inputs(
            "inputs:\n  target:\n    required: true\n  dry-run:\n    type: boolean\n    default: true\n  \
             level:\n    type: choice\n    options: [low, high]\n    default: low\n",
        )
        .unwrap();
        assert_eq!(declared.len(), 3);
        assert_eq!(declared[1].default.as_deref(), Some("true"));
        for invalid in [
            "inputs:\n  x:\n    type: choice\n",
            "inputs:\n  x:\n    type: environment\n",
            "inputs:\n  x:\n    type: boolean\n    default: maybe\n",
            "inputs:\n  x:\n    type: choice\n    options: [a]\n    default: b\n",
            "inputs:\n  9x: {}\n",
            "branches: [main]\n",
        ] {
            assert!(inputs(invalid).is_err(), "{invalid}");
        }
        assert!(parse_inputs(None).unwrap().is_empty());
        assert!(parse_inputs(Some(&Value::Null)).unwrap().is_empty());
    }

    #[test]
    fn provided_inputs_merge_with_defaults() {
        let declared = inputs(
            "inputs:\n  target:\n    required: true\n  dry-run:\n    type: boolean\n  \
             level:\n    type: choice\n    options: [low, high]\n    default: low\n",
        )
        .unwrap();
        let provided = BTreeMap::from([
            ("target".to_owned(), json!("prod")),
            ("dry-run".to_owned(), json!(true)),
        ]);
        let resolved = resolve_inputs(&declared, &provided).unwrap();
        assert_eq!(resolved["target"], "prod");
        assert_eq!(resolved["dry-run"], "true");
        assert_eq!(resolved["level"], "low");

        assert!(resolve_inputs(&declared, &BTreeMap::new()).is_err());
        let unknown = BTreeMap::from([
            ("target".to_owned(), json!("prod")),
            ("other".to_owned(), json!("x")),
        ]);
        assert!(resolve_inputs(&declared, &unknown).is_err());
        let bad_choice = BTreeMap::from([
            ("target".to_owned(), json!("prod")),
            ("level".to_owned(), json!("max")),
        ]);
        assert!(resolve_inputs(&declared, &bad_choice).is_err());
    }
}
