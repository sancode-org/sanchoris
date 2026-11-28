//! The Lean definition of a `skill.yaml` that passed the validation of `sanchoris-config`.

use serde_json::{Map, Value};

use crate::generated::sanchoris_engine::{
    Body, Catcher, Command, Cond, Definition, Num, Prompt, Rel, Retrier, State, Test,
};
use crate::json::{nat, to_lean};

/// A `skill.yaml` that does not have the form the validation checks.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{path}: {message}")]
pub struct DefinitionError {
    pub path: String,
    pub message: String,
}

fn error(path: &str, message: impl Into<String>) -> DefinitionError {
    DefinitionError {
        path: path.to_string(),
        message: message.into(),
    }
}

fn object<'a>(value: &'a Value, path: &str) -> Result<&'a Map<String, Value>, DefinitionError> {
    value
        .as_object()
        .ok_or_else(|| error(path, "not a mapping"))
}

fn string(m: &Map<String, Value>, path: &str, key: &str) -> Result<String, DefinitionError> {
    m.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| error(path, format!("{key} is not a string")))
}

fn optional_string(
    m: &Map<String, Value>,
    path: &str,
    key: &str,
) -> Result<Option<String>, DefinitionError> {
    match m.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(error(path, format!("{key} is not a string"))),
    }
}

fn unsigned(m: &Map<String, Value>, path: &str, key: &str) -> Result<u64, DefinitionError> {
    m.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| error(path, format!("{key} is not a whole number")))
}

fn boolean(m: &Map<String, Value>, path: &str, key: &str) -> Result<bool, DefinitionError> {
    m.get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| error(path, format!("{key} is not a boolean")))
}

fn number(m: &Map<String, Value>, path: &str, key: &str) -> Result<Num, DefinitionError> {
    match m.get(key).map(to_lean) {
        Some(crate::generated::sanchoris_engine::Json::Num { n }) => Ok(n),
        _ => Err(error(path, format!("{key} is not a number"))),
    }
}

fn list<'a>(
    m: &'a Map<String, Value>,
    path: &str,
    key: &str,
) -> Result<&'a [Value], DefinitionError> {
    match m.get(key) {
        None => Ok(&[]),
        Some(Value::Array(items)) => Ok(items),
        Some(_) => Err(error(path, format!("{key} is not a list"))),
    }
}

fn strings(m: &Map<String, Value>, path: &str, key: &str) -> Result<Vec<String>, DefinitionError> {
    list(m, path, key)?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_string)
                .ok_or_else(|| error(path, format!("{key} is not a list of strings")))
        })
        .collect()
}

/// `ResultPath`: `$` when it is missing, `None` for `null`.
fn result_path(m: &Map<String, Value>, path: &str) -> Result<Option<String>, DefinitionError> {
    match m.get("ResultPath") {
        None => Ok(Some("$".to_string())),
        Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(error(path, "ResultPath is not a path or null")),
    }
}

fn prompt(m: &Map<String, Value>, path: &str) -> Result<Prompt, DefinitionError> {
    let mut prompt = Prompt {
        transition: true,
        stdout: true,
        stderr: true,
    };
    if let Some(v) = m.get("Prompt") {
        let p = object(v, &format!("{path}.Prompt"))?;
        let get = |key: &str| match p.get(key) {
            None => Ok(true),
            Some(_) => boolean(p, path, key),
        };
        prompt.transition = get("Transition")?;
        prompt.stdout = get("Stdout")?;
        prompt.stderr = get("Stderr")?;
    }
    Ok(prompt)
}

const RELATIONS: [(&str, Rel); 5] = [
    ("NumericEquals", Rel::Eq),
    ("NumericLessThan", Rel::Lt),
    ("NumericGreaterThan", Rel::Gt),
    ("NumericLessThanEquals", Rel::Le),
    ("NumericGreaterThanEquals", Rel::Ge),
];

fn condition(m: &Map<String, Value>, path: &str) -> Result<Cond, DefinitionError> {
    let conds = |key: &str| -> Result<Vec<Cond>, DefinitionError> {
        list(m, path, key)?
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let p = format!("{path}.{key}[{i}]");
                condition(object(v, &p)?, &p)
            })
            .collect()
    };
    if m.contains_key("And") {
        return Ok(Cond::All {
            conds: conds("And")?,
        });
    }
    if m.contains_key("Or") {
        return Ok(Cond::Any {
            conds: conds("Or")?,
        });
    }
    if let Some(v) = m.get("Not") {
        let p = format!("{path}.Not");
        return Ok(Cond::Not {
            cond: Box::new(condition(object(v, &p)?, &p)?),
        });
    }
    let variable = string(m, path, "Variable")?;
    let test = if m.contains_key("StringEquals") {
        Test::StringEquals {
            s: string(m, path, "StringEquals")?,
        }
    } else if m.contains_key("StringMatches") {
        Test::StringMatches {
            pattern: string(m, path, "StringMatches")?,
        }
    } else if let Some((key, rel)) = RELATIONS.iter().find(|(k, _)| m.contains_key(*k)) {
        Test::Numeric {
            rel: *rel,
            n: number(m, path, key)?,
        }
    } else if m.contains_key("BooleanEquals") {
        Test::BooleanEquals {
            b: boolean(m, path, "BooleanEquals")?,
        }
    } else if m.contains_key("IsPresent") {
        Test::IsPresent {
            b: boolean(m, path, "IsPresent")?,
        }
    } else if m.contains_key("IsNull") {
        Test::IsNull {
            b: boolean(m, path, "IsNull")?,
        }
    } else {
        return Err(error(path, "a Choice rule without an operator"));
    };
    Ok(Cond::Test {
        path: variable,
        test,
    })
}

fn retrier(m: &Map<String, Value>, path: &str) -> Result<Retrier, DefinitionError> {
    Ok(Retrier {
        error_equals: strings(m, path, "ErrorEquals")?,
        interval_seconds: nat(unsigned(m, path, "IntervalSeconds")?),
        max_attempts: nat(unsigned(m, path, "MaxAttempts")?),
        backoff_rate: number(m, path, "BackoffRate")?,
        max_delay_seconds: match m.get("MaxDelaySeconds") {
            None => None,
            Some(_) => Some(nat(unsigned(m, path, "MaxDelaySeconds")?)),
        },
    })
}

fn catcher(m: &Map<String, Value>, path: &str) -> Result<Catcher, DefinitionError> {
    Ok(Catcher {
        error_equals: strings(m, path, "ErrorEquals")?,
        result_path: result_path(m, path)?,
        next: string(m, path, "Next")?,
    })
}

fn items<T>(
    m: &Map<String, Value>,
    path: &str,
    key: &str,
    read: fn(&Map<String, Value>, &str) -> Result<T, DefinitionError>,
) -> Result<Vec<T>, DefinitionError> {
    list(m, path, key)?
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let p = format!("{path}.{key}[{i}]");
            read(object(v, &p)?, &p)
        })
        .collect()
}

fn state(name: &str, value: &Value) -> Result<State, DefinitionError> {
    let path = format!("States.{name}");
    let m = object(value, &path)?;
    let body = match string(m, &path, "Type")?.as_str() {
        "Command" => {
            let (run, run_is_path) = match optional_string(m, &path, "Run.$")? {
                Some(expr) => (expr, true),
                None => (string(m, &path, "Run")?, false),
            };
            Body::Command {
                c: Command {
                    run,
                    run_is_path,
                    timeout_seconds: nat(unsigned(m, &path, "TimeoutSeconds")?),
                    result_path: result_path(m, &path)?,
                    retry: items(m, &path, "Retry", retrier)?,
                    catchers: items(m, &path, "Catch", catcher)?,
                },
            }
        }
        "Pass" => Body::Pass {
            result: m.get("Result").map(to_lean),
            parameters: m.get("Parameters").map(to_lean),
            result_path: result_path(m, &path)?,
        },
        "Choice" => Body::Choice {
            choices: list(m, &path, "Choices")?
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let p = format!("{path}.Choices[{i}]");
                    let rule = object(v, &p)?;
                    Ok((condition(rule, &p)?, string(rule, &p, "Next")?))
                })
                .collect::<Result<_, DefinitionError>>()?,
            default: optional_string(m, &path, "Default")?,
        },
        "Wait" => match optional_string(m, &path, "Timestamp")? {
            Some(timestamp) => Body::WaitUntil { timestamp },
            None => Body::WaitSeconds {
                seconds: nat(unsigned(m, &path, "Seconds")?),
            },
        },
        "Succeed" => Body::Succeed,
        "Fail" => Body::Fail {
            error: string(m, &path, "Error")?,
            cause: string(m, &path, "Cause")?,
        },
        other => return Err(error(&path, format!("{other} is not a State type"))),
    };
    let next = match m.get("End") {
        Some(Value::Bool(true)) => None,
        _ => optional_string(m, &path, "Next")?,
    };
    Ok(State {
        name: name.to_string(),
        body,
        next,
        prompt: prompt(m, &path)?,
    })
}

/// The definition of the skill `skill` from the `source` of a validated `skill.yaml`.
pub fn definition(skill: &str, source: &Value) -> Result<Definition, DefinitionError> {
    let root = object(source, "")?;
    let states = object(
        root.get("States").ok_or_else(|| error("", "no States"))?,
        "States",
    )?;
    Ok(Definition {
        skill: skill.to_string(),
        start_at: string(root, "", "StartAt")?,
        timeout_seconds: nat(unsigned(root, "", "TimeoutSeconds")?),
        states: states
            .iter()
            .map(|(name, v)| state(name, v))
            .collect::<Result<_, _>>()?,
    })
}
