//! The format of an `EventPattern`: an EventBridge event pattern with exact values, `prefix`,
//! `anything-but`, `exists` and `numeric`, over the GitHub events of sanchoris.

use serde_json::{Map, Value};

use crate::issue::{index, join};

/// The `detail-type`s of the `github` source.
pub const DETAIL_TYPES: [&str; 3] = [
    "Issue Labeled",
    "Issue Comment Created",
    "Check Run Completed",
];

const NUMERIC_OPERATORS: [&str; 5] = ["<", "<=", "=", ">=", ">"];

/// The format issues of an event pattern, as paths below `path` and messages.
pub fn check(pattern: &Value, path: &str) -> Vec<(String, String)> {
    let mut issues = Vec::new();
    let Some(fields) = pattern.as_object().filter(|m| !m.is_empty()) else {
        issues.push((path.to_string(), "must be a non-empty mapping".to_string()));
        return issues;
    };
    fields_of(fields, path, &mut issues);
    if let Some(sources) = fields.get("source").and_then(Value::as_array) {
        for (i, source) in sources.iter().enumerate() {
            if source.as_str() != Some("github") {
                issues.push((
                    index(&join(path, "source"), i),
                    "the event source is github".to_string(),
                ));
            }
        }
    }
    if let Some(types) = fields.get("detail-type").and_then(Value::as_array) {
        for (i, t) in types.iter().enumerate() {
            if !t.as_str().is_some_and(|t| DETAIL_TYPES.contains(&t)) {
                issues.push((
                    index(&join(path, "detail-type"), i),
                    format!("must be one of {}", DETAIL_TYPES.join(", ")),
                ));
            }
        }
    }
    let repository_path = join(&join(path, "detail"), "repository");
    match fields.get("detail").and_then(|d| d.get("repository")) {
        None => issues.push((
            repository_path,
            "missing required item: the list of repositories".to_string(),
        )),
        Some(repository) => {
            if repositories(repository).is_none() {
                issues.push((
                    repository_path,
                    "must be a list of exact owner/name values".to_string(),
                ));
            }
        }
    }
    issues
}

/// The `owner/name` values of `detail.repository` when it is a list of exact names.
pub fn repositories(value: &Value) -> Option<Vec<&str>> {
    let items = value.as_array().filter(|items| !items.is_empty())?;
    items
        .iter()
        .map(|v| v.as_str().filter(|s| is_owner_name(s)))
        .collect()
}

pub(crate) fn is_owner_name(s: &str) -> bool {
    let part = |p: &str| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    matches!(s.split_once('/'), Some((owner, name)) if part(owner) && part(name))
}

fn fields_of(fields: &Map<String, Value>, path: &str, issues: &mut Vec<(String, String)>) {
    for (key, value) in fields {
        let path = join(path, key);
        match value {
            Value::Object(inner) if !inner.is_empty() => fields_of(inner, &path, issues),
            Value::Array(matchers) if !matchers.is_empty() => {
                for (i, m) in matchers.iter().enumerate() {
                    if let Err(message) = matcher(m) {
                        issues.push((index(&path, i), message.to_string()));
                    }
                }
            }
            _ => issues.push((
                path,
                "must be a non-empty list of values or a mapping of fields".to_string(),
            )),
        }
    }
}

fn is_scalar(v: &Value) -> bool {
    matches!(
        v,
        Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null
    )
}

fn matcher(m: &Value) -> Result<(), &'static str> {
    if is_scalar(m) {
        return Ok(());
    }
    let Some(object) = m.as_object().filter(|o| o.len() == 1) else {
        return Err("must be a value or one of {prefix}, {anything-but}, {exists} and {numeric}");
    };
    let (key, value) = object.iter().next().ok_or("must not be empty")?;
    match key.as_str() {
        "prefix" => value
            .is_string()
            .then_some(())
            .ok_or("prefix takes a string"),
        "exists" => value
            .is_boolean()
            .then_some(())
            .ok_or("exists takes true or false"),
        "anything-but" => {
            let ok = match value {
                Value::Array(items) => !items.is_empty() && items.iter().all(is_scalar),
                Value::Object(o) => o.len() == 1 && o.get("prefix").is_some_and(Value::is_string),
                v => is_scalar(v),
            };
            ok.then_some(())
                .ok_or("anything-but takes a value, a list of values or {prefix}")
        }
        "numeric" => {
            let ok = value.as_array().is_some_and(|items| {
                (items.len() == 2 || items.len() == 4)
                    && items.chunks(2).all(|pair| {
                        pair[0]
                            .as_str()
                            .is_some_and(|op| NUMERIC_OPERATORS.contains(&op))
                            && pair[1].is_number()
                    })
            });
            ok.then_some(())
                .ok_or("numeric takes one or two pairs of an operator and a number")
        }
        _ => Err("must be a value or one of {prefix}, {anything-but}, {exists} and {numeric}"),
    }
}
