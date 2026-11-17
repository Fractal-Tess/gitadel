//! Static `strategy.matrix` expansion following GitHub Actions semantics.
//!
//! The cartesian product of the matrix keys is reduced by `exclude`, then
//! each `include` entry extends every combination whose original values it
//! does not overwrite, or is appended as a new combination when it extends
//! none. Expressions are not evaluated, so matrices must be static.

use serde::{Deserialize, Serialize};
use yaml_serde::{Mapping, Value};

/// GitHub's limit on combinations generated for one job.
pub(crate) const MAX_COMBINATIONS: usize = 256;

/// One combination, keeping the declaration order of its keys.
pub(crate) type Combination = Vec<(String, Value)>;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Strategy {
    pub(crate) combinations: Vec<Combination>,
    pub(crate) fail_fast: bool,
    pub(crate) max_parallel: Option<u32>,
}

/// Scheduling data stored with every expanded job.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub(crate) struct StoredMatrix {
    pub(crate) values: serde_json::Map<String, serde_json::Value>,
    pub(crate) fail_fast: bool,
    pub(crate) max_parallel: Option<u32>,
}

impl StoredMatrix {
    pub(crate) fn parse(raw: Option<&str>) -> Option<Self> {
        serde_json::from_str(raw?).ok()
    }
}

/// The workflow job ID of a stored job key (`build:3` -> `build`).
pub(crate) fn base_job_key(key: &str) -> &str {
    key.split_once(':').map_or(key, |(base, _)| base)
}

/// Parses a job's `strategy`. `Ok(None)` means the job has no matrix.
pub(crate) fn parse_strategy(job: &Mapping) -> Result<Option<Strategy>, String> {
    let Some(strategy) = job.get("strategy") else {
        return Ok(None);
    };
    let strategy = strategy
        .as_mapping()
        .ok_or_else(|| "`strategy` must be a mapping".to_owned())?;
    if let Some(key) = strategy
        .keys()
        .find(|key| !matches!(key.as_str(), Some("matrix" | "fail-fast" | "max-parallel")))
    {
        return Err(format!(
            "`strategy` contains unsupported key `{}`",
            key.as_str().unwrap_or("?")
        ));
    }
    let fail_fast = match strategy.get("fail-fast") {
        None => true,
        Some(Value::Bool(value)) => *value,
        Some(_) => return Err("`strategy.fail-fast` must be a static boolean".to_owned()),
    };
    let max_parallel = match strategy.get("max-parallel") {
        None => None,
        Some(value) => Some(
            value
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value > 0)
                .ok_or_else(|| "`strategy.max-parallel` must be a positive integer".to_owned())?,
        ),
    };
    let Some(matrix) = strategy.get("matrix") else {
        return Ok(None);
    };
    let matrix = matrix
        .as_mapping()
        .ok_or_else(|| "`strategy.matrix` must be a static mapping".to_owned())?;
    let combinations = expand(matrix)?;
    Ok(Some(Strategy {
        combinations,
        fail_fast,
        max_parallel,
    }))
}

fn contains_expression(value: &Value) -> bool {
    match value {
        Value::String(text) => text.contains("${{"),
        Value::Sequence(values) => values.iter().any(contains_expression),
        Value::Mapping(map) => map
            .iter()
            .any(|(key, value)| contains_expression(key) || contains_expression(value)),
        Value::Tagged(tagged) => contains_expression(&tagged.value),
        _ => false,
    }
}

fn entries(value: Option<&Value>, name: &str) -> Result<Vec<Combination>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let sequence = value
        .as_sequence()
        .ok_or_else(|| format!("`matrix.{name}` must be a list of mappings"))?;
    sequence
        .iter()
        .map(|entry| {
            let entry = entry
                .as_mapping()
                .ok_or_else(|| format!("`matrix.{name}` must be a list of mappings"))?;
            entry
                .iter()
                .map(|(key, value)| {
                    let key = key
                        .as_str()
                        .ok_or_else(|| format!("`matrix.{name}` keys must be strings"))?;
                    Ok((key.to_owned(), value.clone()))
                })
                .collect()
        })
        .collect()
}

pub(crate) fn expand(matrix: &Mapping) -> Result<Vec<Combination>, String> {
    if contains_expression(&Value::Mapping(matrix.clone())) {
        return Err("`strategy.matrix` must not use expressions".to_owned());
    }
    let include = entries(matrix.get("include"), "include")?;
    let exclude = entries(matrix.get("exclude"), "exclude")?;
    let mut dimensions = Vec::new();
    for (key, values) in matrix {
        let key = key
            .as_str()
            .ok_or_else(|| "matrix keys must be strings".to_owned())?;
        if matches!(key, "include" | "exclude") {
            continue;
        }
        let values = values
            .as_sequence()
            .ok_or_else(|| format!("matrix key `{key}` must be a list"))?;
        if values.is_empty() {
            return Err(format!("matrix key `{key}` must not be empty"));
        }
        dimensions.push((key.to_owned(), values.clone()));
    }

    let mut product: Vec<Combination> = if dimensions.is_empty() {
        Vec::new()
    } else {
        vec![Vec::new()]
    };
    for (key, values) in &dimensions {
        let mut next = Vec::with_capacity(product.len() * values.len());
        for combination in &product {
            for value in values {
                let mut extended = combination.clone();
                extended.push((key.clone(), value.clone()));
                next.push(extended);
            }
            if next.len() > MAX_COMBINATIONS {
                return Err(format!("matrix exceeds {MAX_COMBINATIONS} combinations"));
            }
        }
        product = next;
    }

    product.retain(|combination| {
        !exclude.iter().any(|excluded| {
            excluded.iter().all(|(key, value)| {
                combination
                    .iter()
                    .any(|(candidate, current)| candidate == key && current == value)
            })
        })
    });

    let original_keys: Vec<&str> = dimensions.iter().map(|(key, _)| key.as_str()).collect();
    let original_count = product.len();
    let mut appended = Vec::new();
    for entry in include {
        let mut matched = false;
        for combination in product.iter_mut().take(original_count) {
            let compatible = entry.iter().all(|(key, value)| {
                !original_keys.contains(&key.as_str())
                    || combination
                        .iter()
                        .any(|(candidate, current)| candidate == key && current == value)
            });
            if !compatible {
                continue;
            }
            matched = true;
            for (key, value) in &entry {
                match combination
                    .iter_mut()
                    .find(|(candidate, _)| candidate == key)
                {
                    Some((_, current)) => *current = value.clone(),
                    None => combination.push((key.clone(), value.clone())),
                }
            }
        }
        if !matched {
            appended.push(entry);
        }
    }
    product.extend(appended);
    if product.is_empty() {
        return Err("matrix produces no combinations".to_owned());
    }
    if product.len() > MAX_COMBINATIONS {
        return Err(format!("matrix exceeds {MAX_COMBINATIONS} combinations"));
    }
    Ok(product)
}

/// Renders a matrix value the way GitHub shows it in job names.
pub(crate) fn display_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Null => String::new(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// Replaces `${{ matrix.KEY }}` placeholders with the combination's values.
/// Returns `None` when an expression other than a known matrix key remains.
pub(crate) fn substitute(text: &str, combination: &Combination) -> Option<String> {
    let mut output = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${{") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 3..];
        let end = after.find("}}")?;
        let expression = after[..end].trim();
        let key = expression.strip_prefix("matrix.")?;
        let (_, value) = combination.iter().find(|(candidate, _)| candidate == key)?;
        output.push_str(&display_value(value));
        rest = &after[end + 2..];
    }
    output.push_str(rest);
    Some(output)
}

/// Name of one expansion: an explicit templated name is interpolated,
/// otherwise the combination's values are appended like `build (a, b)`.
pub(crate) fn job_name(base: &str, explicit: bool, combination: &Combination) -> String {
    if explicit && base.contains("${{") {
        return substitute(base, combination).unwrap_or_else(|| base.to_owned());
    }
    let values: Vec<String> = combination
        .iter()
        .map(|(_, value)| display_value(value))
        .collect();
    format!("{base} ({})", values.join(", "))
}

/// The job's `strategy` rewritten so the runner sees exactly this combination
/// as its `matrix` context.
pub(crate) fn pinned_strategy(original: &Mapping, combination: &Combination) -> Mapping {
    let mut strategy = original.clone();
    let mut matrix = Mapping::new();
    for (key, value) in combination {
        matrix.insert(
            Value::String(key.clone()),
            Value::Sequence(vec![value.clone()]),
        );
    }
    strategy.insert(Value::String("matrix".to_owned()), Value::Mapping(matrix));
    strategy
}

pub(crate) fn stored(strategy: &Strategy, combination: &Combination) -> StoredMatrix {
    StoredMatrix {
        values: combination
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    serde_json::to_value(value).unwrap_or(serde_json::Value::Null),
                )
            })
            .collect(),
        fail_fast: strategy.fail_fast,
        max_parallel: strategy.max_parallel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matrix(source: &str) -> Mapping {
        yaml_serde::from_str::<Value>(source)
            .unwrap()
            .as_mapping()
            .unwrap()
            .clone()
    }

    fn render(combinations: &[Combination]) -> Vec<String> {
        combinations
            .iter()
            .map(|combination| {
                combination
                    .iter()
                    .map(|(key, value)| format!("{key}={}", display_value(value)))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect()
    }

    #[test]
    fn product_respects_declaration_order() {
        let combinations = expand(&matrix("os: [linux, mac]\nnode: [18, 20]\n")).unwrap();
        assert_eq!(
            render(&combinations),
            [
                "os=linux,node=18",
                "os=linux,node=20",
                "os=mac,node=18",
                "os=mac,node=20"
            ]
        );
    }

    #[test]
    fn exclude_and_include_follow_github_rules() {
        let combinations = expand(&matrix(
            "fruit: [apple, pear]\nanimal: [cat, dog]\n\
             exclude:\n  - fruit: pear\n    animal: dog\n\
             include:\n  - color: green\n  - color: pink\n    animal: cat\n  \
             - fruit: apple\n    shape: circle\n  - fruit: banana\n  \
             - fruit: banana\n    animal: cat\n",
        ))
        .unwrap();
        assert_eq!(
            render(&combinations),
            [
                "fruit=apple,animal=cat,color=pink,shape=circle",
                "fruit=apple,animal=dog,color=green,shape=circle",
                "fruit=pear,animal=cat,color=pink",
                "fruit=banana",
                "fruit=banana,animal=cat",
            ]
        );
    }

    #[test]
    fn include_only_matrices_are_supported() {
        let combinations = expand(&matrix("include:\n  - os: linux\n  - os: windows\n")).unwrap();
        assert_eq!(render(&combinations), ["os=linux", "os=windows"]);
    }

    #[test]
    fn invalid_matrices_are_rejected() {
        for source in [
            "os: linux\n",
            "os: []\n",
            "os: ['${{ fromJSON(x) }}']\n",
            "os: [a]\nexclude:\n  - os: a\n",
        ] {
            assert!(expand(&matrix(source)).is_err(), "{source}");
        }
        let wide = format!(
            "a: [{}]\nb: [{}]\n",
            (0..20).map(|n| n.to_string()).collect::<Vec<_>>().join(","),
            (0..20).map(|n| n.to_string()).collect::<Vec<_>>().join(",")
        );
        assert!(expand(&matrix(&wide)).is_err());
    }

    #[test]
    fn strategy_options_are_parsed() {
        let job =
            matrix("strategy:\n  fail-fast: false\n  max-parallel: 2\n  matrix:\n    os: [a]\n");
        let strategy = parse_strategy(&job).unwrap().unwrap();
        assert!(!strategy.fail_fast);
        assert_eq!(strategy.max_parallel, Some(2));
        let job = matrix("strategy:\n  max-parallel: 0\n  matrix:\n    os: [a]\n");
        assert!(parse_strategy(&job).is_err());
        assert!(parse_strategy(&matrix("runs-on: x\n")).unwrap().is_none());
    }

    #[test]
    fn names_and_labels_interpolate_matrix_values() {
        let combination = vec![
            ("os".to_owned(), Value::String("linux".to_owned())),
            (
                "node".to_owned(),
                yaml_serde::from_str::<Value>("20").unwrap(),
            ),
        ];
        assert_eq!(job_name("build", false, &combination), "build (linux, 20)");
        assert_eq!(
            job_name("Test on ${{ matrix.os }}", true, &combination),
            "Test on linux"
        );
        assert_eq!(
            substitute("${{matrix.os}}-runner", &combination).as_deref(),
            Some("linux-runner")
        );
        assert!(substitute("${{ github.ref }}", &combination).is_none());
    }
}
