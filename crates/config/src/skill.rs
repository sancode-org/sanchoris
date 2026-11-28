use indexmap::IndexMap;
use serde_yaml_ng::{Mapping, Value};

use crate::generated::sanchoris_skill_definition as lean;
use crate::issue::{Check, index, join};
use crate::reader::Reader;

/// The type of an item of `Inputs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputType {
    String,
    Number,
    Boolean,
    Array,
    Object,
}

const INPUT_TYPES: [(&str, InputType); 5] = [
    ("string", InputType::String),
    ("number", InputType::Number),
    ("boolean", InputType::Boolean),
    ("array", InputType::Array),
    ("object", InputType::Object),
];

const STATE_TYPES: [(&str, lean::StateType); 6] = [
    ("Command", lean::StateType::Command),
    ("Pass", lean::StateType::Pass),
    ("Choice", lean::StateType::Choice),
    ("Wait", lean::StateType::Wait),
    ("Succeed", lean::StateType::Succeed),
    ("Fail", lean::StateType::Fail),
];

const STRING_COMPARISONS: [&str; 2] = ["StringEquals", "StringMatches"];
const NUMERIC_COMPARISONS: [&str; 5] = [
    "NumericEquals",
    "NumericLessThan",
    "NumericGreaterThan",
    "NumericLessThanEquals",
    "NumericGreaterThanEquals",
];
const BOOLEAN_COMPARISONS: [&str; 3] = ["BooleanEquals", "IsPresent", "IsNull"];
const CATCHABLE_STATES_ERRORS: [&str; 3] =
    ["States.ALL", "States.Timeout", "States.NoChoiceMatched"];

/// A `skill.yaml` that passed validation.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillDefinition {
    pub inputs: IndexMap<String, InputType>,
    /// The whole definition, for the engine.
    pub source: serde_json::Value,
}

/// Reads a `skill.yaml`, checks the formats of its values, and runs the Lean checks of the
/// required keys, the references to States, reachability, the terminal State and the place of
/// `States.ALL`.
pub(crate) fn read(r: &mut Reader, text: &str) -> Option<SkillDefinition> {
    let value = r.parse(text)?;
    let root = r.mapping(&value, "")?;
    let keys = keys(root);
    r.string(root.get("Comment"), "Comment");
    let mut inputs = IndexMap::new();
    if let Some(v) = root.get("Inputs")
        && let Some(m) = r.mapping(v, "Inputs")
    {
        for (name, v) in r.entries(m, "Inputs") {
            if let Some(t) = r.one_of(Some(v), &join("Inputs", &name), &INPUT_TYPES) {
                inputs.insert(name, t);
            }
        }
    }
    let start_at = r.string(root.get("StartAt"), "StartAt");
    r.integer(root.get("TimeoutSeconds"), "TimeoutSeconds", 1);
    let mut states = Vec::new();
    if let Some(v) = root.get("States")
        && let Some(m) = r.mapping(v, "States")
    {
        for (name, v) in r.entries(m, "States") {
            states.push(state(r, &name, v));
        }
    }
    if let Some(v) = root.get("Layout")
        && let Some(m) = r.mapping(v, "Layout")
    {
        for (name, v) in r.entries(m, "Layout") {
            let path = join("Layout", &name);
            if !states.iter().any(|s| s.name == name) {
                r.format(&path, format!("{name} names no State"));
            }
            if let Some(m) = r.mapping(v, &path) {
                for axis in ["x", "y"] {
                    let v = r.get(m, &path, axis, true);
                    r.number(v, &join(&path, axis));
                }
            }
        }
    }
    let definition = lean::Definition {
        keys,
        start_at,
        states,
    };
    let issues = lean::validate(definition);
    // Without a State to start from, every State is unreachable; the issue of StartAt says it.
    let no_start = issues.iter().any(|issue| match issue {
        lean::Issue::MissingField { place, fields } => {
            *place == lean::Place::Root && fields == &["StartAt"]
        }
        lean::Issue::UnknownState { place, .. } => *place == lean::Place::Root,
        _ => false,
    });
    for issue in issues {
        if !(no_start && matches!(issue, lean::Issue::Unreachable { .. })) {
            push_lean_issue(r, issue);
        }
    }
    Some(SkillDefinition {
        inputs,
        source: r.json(&value, "")?,
    })
}

fn keys(m: &Mapping) -> Vec<String> {
    m.keys()
        .filter_map(|k| k.as_str().map(str::to_string))
        .collect()
}

fn state(r: &mut Reader, name: &str, value: &Value) -> lean::State {
    let path = join("States", name);
    let mut state = lean::State {
        name: name.to_string(),
        keys: Vec::new(),
        r#type: None,
        next: None,
        is_end: false,
        choices: Vec::new(),
        default: None,
        retriers: Vec::new(),
        catchers: Vec::new(),
    };
    let Some(m) = r.mapping(value, &path) else {
        return state;
    };
    state.keys = keys(m);
    state.r#type = r.one_of(m.get("Type"), &join(&path, "Type"), &STATE_TYPES);
    state.next = r.string(m.get("Next"), &join(&path, "Next"));
    state.is_end = r.boolean(m.get("End"), &join(&path, "End")) == Some(true);
    state.default = r.string(m.get("Default"), &join(&path, "Default"));
    r.string(m.get("Comment"), &join(&path, "Comment"));
    prompt(r, m, &path);
    match state.r#type {
        Some(lean::StateType::Command) => {
            let run = r.string(m.get("Run"), &join(&path, "Run"));
            let run_path = r.string(m.get("Run.$"), &join(&path, "Run.$"));
            if run.is_some() && run_path.is_some() {
                r.format(&path, "has both Run and Run.$");
            }
            r.integer(m.get("TimeoutSeconds"), &join(&path, "TimeoutSeconds"), 1);
            result_path(r, m, &path);
            state.retriers = rules(r, m, &path, "Retry", retrier);
            state.catchers = rules(r, m, &path, "Catch", catcher);
        }
        Some(lean::StateType::Pass) => {
            if let Some(v) = m.get("Parameters") {
                r.mapping(v, &join(&path, "Parameters"));
            }
            result_path(r, m, &path);
        }
        Some(lean::StateType::Wait) => {
            let seconds = r.integer(m.get("Seconds"), &join(&path, "Seconds"), 0);
            let timestamp = r.string(m.get("Timestamp"), &join(&path, "Timestamp"));
            if seconds.is_some() && timestamp.is_some() {
                r.format(&path, "has both Seconds and Timestamp");
            }
        }
        Some(lean::StateType::Choice) => {
            state.choices = rules(r, m, &path, "Choices", choice);
            if m.get("Choices")
                .and_then(Value::as_sequence)
                .is_some_and(Vec::is_empty)
            {
                r.format(&join(&path, "Choices"), "must not be empty");
            }
        }
        Some(lean::StateType::Fail) => {
            let error_path = join(&path, "Error");
            if let Some(error) = r.string(m.get("Error"), &error_path)
                && error.starts_with("States.")
            {
                r.format(
                    &error_path,
                    "an error of a skill must not start with States.",
                );
            }
            r.string(m.get("Cause"), &join(&path, "Cause"));
        }
        Some(lean::StateType::Succeed) | None => {}
    }
    state
}

fn prompt(r: &mut Reader, m: &Mapping, path: &str) {
    let path = join(path, "Prompt");
    let Some(v) = m.get("Prompt") else { return };
    let Some(m) = r.mapping(v, &path) else { return };
    for (key, v) in r.entries(m, &path) {
        let key_path = join(&path, &key);
        if ["Transition", "Stdout", "Stderr"].contains(&key.as_str()) {
            r.boolean(Some(v), &key_path);
        } else {
            r.format(&key_path, "Prompt takes Transition, Stdout and Stderr");
        }
    }
}

fn result_path(r: &mut Reader, m: &Mapping, path: &str) {
    match m.get("ResultPath") {
        None | Some(Value::Null) | Some(Value::String(_)) => {}
        Some(_) => r.format(&join(path, "ResultPath"), "must be a path or null"),
    }
}

fn rules(
    r: &mut Reader,
    m: &Mapping,
    path: &str,
    key: &str,
    read: fn(&mut Reader, &Mapping, &str) -> lean::Rule,
) -> Vec<lean::Rule> {
    let path = join(path, key);
    let Some(v) = m.get(key) else {
        return Vec::new();
    };
    let Some(items) = v.as_sequence() else {
        r.format(&path, "must be a list");
        return Vec::new();
    };
    items
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let item_path = index(&path, i);
            match r.mapping(v, &item_path) {
                Some(m) => read(r, m, &item_path),
                None => lean::Rule {
                    keys: Vec::new(),
                    error_equals: Vec::new(),
                    next: None,
                },
            }
        })
        .collect()
}

fn error_equals(r: &mut Reader, m: &Mapping, path: &str) -> Vec<String> {
    let path = join(path, "ErrorEquals");
    let Some(v) = m.get("ErrorEquals") else {
        return Vec::new();
    };
    let Some(items) = v.as_sequence().filter(|items| !items.is_empty()) else {
        r.format(&path, "must be a non-empty list of error names");
        return Vec::new();
    };
    let mut names = Vec::new();
    for (i, v) in items.iter().enumerate() {
        let item_path = index(&path, i);
        if let Some(name) = r.string(Some(v), &item_path) {
            if name.starts_with("States.") && !CATCHABLE_STATES_ERRORS.contains(&name.as_str()) {
                r.format(&item_path, format!("{name} cannot be retried or caught"));
            }
            names.push(name);
        }
    }
    names
}

fn retrier(r: &mut Reader, m: &Mapping, path: &str) -> lean::Rule {
    r.integer(m.get("IntervalSeconds"), &join(path, "IntervalSeconds"), 1);
    r.integer(m.get("MaxAttempts"), &join(path, "MaxAttempts"), 0);
    r.integer(m.get("MaxDelaySeconds"), &join(path, "MaxDelaySeconds"), 1);
    let backoff_path = join(path, "BackoffRate");
    if let Some(rate) = r.number(m.get("BackoffRate"), &backoff_path)
        && rate < 1.0
    {
        r.format(&backoff_path, "must be at least 1");
    }
    lean::Rule {
        keys: keys(m),
        error_equals: error_equals(r, m, path),
        next: None,
    }
}

fn catcher(r: &mut Reader, m: &Mapping, path: &str) -> lean::Rule {
    result_path(r, m, path);
    lean::Rule {
        keys: keys(m),
        error_equals: error_equals(r, m, path),
        next: r.string(m.get("Next"), &join(path, "Next")),
    }
}

fn choice(r: &mut Reader, m: &Mapping, path: &str) -> lean::Rule {
    condition(r, m, path);
    lean::Rule {
        keys: keys(m),
        error_equals: Vec::new(),
        next: r.string(m.get("Next"), &join(path, "Next")),
    }
}

/// A Choice rule has one operator: `And`, `Or` or `Not`, or a comparison with `Variable`.
fn condition(r: &mut Reader, m: &Mapping, path: &str) {
    let operators: Vec<String> = keys(m)
        .into_iter()
        .filter(|k| {
            ["And", "Or", "Not"].contains(&k.as_str())
                || STRING_COMPARISONS.contains(&k.as_str())
                || NUMERIC_COMPARISONS.contains(&k.as_str())
                || BOOLEAN_COMPARISONS.contains(&k.as_str())
        })
        .collect();
    let [operator] = operators.as_slice() else {
        r.format(path, "a Choice rule takes exactly one operator");
        return;
    };
    let operator_path = join(path, operator);
    let value = m.get(operator.as_str());
    match operator.as_str() {
        "And" | "Or" => match value.and_then(Value::as_sequence) {
            Some(items) if !items.is_empty() => {
                for (i, v) in items.iter().enumerate() {
                    let item_path = index(&operator_path, i);
                    if let Some(m) = r.mapping(v, &item_path) {
                        condition(r, m, &item_path);
                    }
                }
            }
            _ => r.format(&operator_path, "must be a non-empty list of rules"),
        },
        "Not" => {
            if let Some(v) = value
                && let Some(m) = r.mapping(v, &operator_path)
            {
                condition(r, m, &operator_path);
            }
        }
        op => {
            let variable_path = join(path, "Variable");
            match r.string(m.get("Variable"), &variable_path) {
                Some(v) if v.starts_with('$') => {}
                Some(_) => r.format(&variable_path, "must be a path starting with $"),
                None => r.push(Check::Required, &variable_path, "missing required item"),
            }
            if STRING_COMPARISONS.contains(&op) {
                r.string(value, &operator_path);
            } else if NUMERIC_COMPARISONS.contains(&op) {
                r.number(value, &operator_path);
            } else {
                r.boolean(value, &operator_path);
            }
        }
    }
}

fn place_path(place: &lean::Place) -> String {
    fn nth(state: &str, key: &str, i: &leanfran::Nat) -> String {
        format!("{}[{i}]", join(&join("States", state), key))
    }
    match place {
        lean::Place::Root => String::new(),
        lean::Place::State { name } => join("States", name),
        lean::Place::Choice { state, index } => nth(state, "Choices", index),
        lean::Place::Retrier { state, index } => nth(state, "Retry", index),
        lean::Place::Catcher { state, index } => nth(state, "Catch", index),
    }
}

fn push_lean_issue(r: &mut Reader, issue: lean::Issue) {
    match issue {
        lean::Issue::MissingField { place, fields } => {
            let path = place_path(&place);
            let message = if fields == ["Next", "End"] {
                "missing required item: Next or End: true".to_string()
            } else {
                format!("missing required item: {}", fields.join(" or "))
            };
            let path = match fields.as_slice() {
                [field] => join(&path, field),
                _ => path,
            };
            r.push(Check::Required, &path, message);
        }
        lean::Issue::UnknownState {
            place,
            field,
            target,
        } => r.push(
            Check::SkillDefinition,
            &join(&place_path(&place), &field),
            format!("{target} names no State"),
        ),
        lean::Issue::Unreachable { state } => r.push(
            Check::SkillDefinition,
            &join("States", &state),
            "not reachable from StartAt",
        ),
        lean::Issue::NoTerminal => r.push(
            Check::SkillDefinition,
            "States",
            "no terminal State (End: true, Succeed or Fail)",
        ),
        lean::Issue::MisplacedStatesAll { place } => r.push(
            Check::SkillDefinition,
            &join(&place_path(&place), "ErrorEquals"),
            "States.ALL must be alone and in the last element",
        ),
    }
}
