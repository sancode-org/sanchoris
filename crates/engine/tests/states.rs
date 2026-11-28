//! The State types, paths, intrinsic functions and errors of `skill.yaml`.

mod common;

use common::*;
use sanchoris_engine::json::{from_lean, u64_of};
use sanchoris_engine::lean::{EntryStatus, RunStatus};
use sanchoris_engine::notice::OUTPUT_TAIL_BYTES;
use serde_json::{Value, json};

fn run_skill(yaml: &str, input: Value, script: Script) -> Sim {
    let mut sim = Sim::new(
        skills(&[("test-skill", yaml)]),
        script,
        Box::new(|_, _| Vec::new()),
    );
    sim.start("test-skill", input);
    sim.run();
    sim
}

fn output(sim: &Sim) -> Value {
    from_lean(sim.run_record(1).run.output.as_ref().expect("an Output"))
}

fn echo() -> Script {
    Box::new(|call: &Call| ok(10, &format!("{}\n", call.request.command)))
}

#[test]
fn pass_parameters_and_intrinsic_functions() {
    let yaml = r#"Inputs: { Repo: string, Count: number }
StartAt: Build
TimeoutSeconds: 60
States:
  Build:
    Type: Pass
    Parameters:
      Text.$: "States.Format('{} has {} items: {}', $.Repo, $.Count, $.Tags)"
      Sum.$: "States.MathAdd($.Count, -3)"
      Encoded.$: States.JsonToString($.Tags)
      Decoded.$: "States.StringToJson('{\"a\": [1, 2.5]}')"
      Name.$: $$.State.Name
      RunId.$: $$.Execution.Id
      Nested:
        - First.$: $.Tags[0]
        - literal
    ResultPath: $.built
    Next: Keep
  Keep:
    Type: Pass
    Result: { Fixed: 1 }
    ResultPath: $.built.result
    End: true
"#;
    let sim = run_skill(
        yaml,
        json!({"Repo": "frontend", "Count": 5, "Tags": ["x", "y"]}),
        echo(),
    );
    assert_eq!(sim.run_record(1).run.status, RunStatus::Succeeded);
    assert_eq!(
        output(&sim)["built"],
        json!({
            "Text": "frontend has 5 items: [\"x\",\"y\"]",
            "Sum": 2,
            "Encoded": "[\"x\",\"y\"]",
            "Decoded": {"a": [1, 2.5]},
            "Name": "Build",
            "RunId": 1,
            "Nested": [{"First": "x"}, "literal"],
            "result": {"Fixed": 1}
        })
    );
}

fn choice_skill(rule: &str) -> String {
    format!(
        "Inputs: {{}}
StartAt: Decide
TimeoutSeconds: 60
States:
  Decide:
    Type: Choice
    Choices:
      - {rule}
        Next: Yes
    Default: No
  Yes:
    Type: Succeed
  No:
    Type: Fail
    Error: Test.No
    Cause: no rule matched
"
    )
}

fn decides(rule: &str, input: Value) -> bool {
    let sim = run_skill(&choice_skill(rule), input, echo());
    match sim.run_record(1).run.status {
        RunStatus::Succeeded => true,
        RunStatus::Failed => {
            assert_eq!(sim.run_record(1).run.error.as_deref(), Some("Test.No"));
            false
        }
        other => panic!("the SkillRun is {other:?}"),
    }
}

#[test]
fn choice_rules() {
    let input = json!({"s": "feature/login", "n": 3, "f": 2.5, "b": true, "z": null});
    let cases: [(&str, bool); 18] = [
        ("Variable: $.s\n        StringEquals: feature/login", true),
        ("Variable: $.s\n        StringEquals: feature", false),
        ("Variable: $.s\n        StringMatches: 'feature/*'", true),
        ("Variable: $.s\n        StringMatches: '*/log*n'", true),
        ("Variable: $.s\n        StringMatches: 'fix/*'", false),
        ("Variable: $.n\n        NumericEquals: 3.0", true),
        ("Variable: $.f\n        NumericGreaterThan: 2", true),
        ("Variable: $.f\n        NumericLessThanEquals: 2.5", true),
        ("Variable: $.f\n        NumericLessThan: 2.5", false),
        ("Variable: $.n\n        NumericGreaterThanEquals: 4", false),
        ("Variable: $.s\n        NumericEquals: 3", false),
        ("Variable: $.b\n        BooleanEquals: true", true),
        ("Variable: $.z\n        IsNull: true", true),
        ("Variable: $.missing\n        IsPresent: false", true),
        ("Variable: $.n\n        IsPresent: false", false),
        (
            "And:\n          - Variable: $.n\n            NumericEquals: 3\n          - Not:\n              Variable: $.b\n              BooleanEquals: false",
            true,
        ),
        (
            "Or:\n          - Variable: $.n\n            NumericEquals: 4\n          - Variable: $.s\n            StringEquals: x",
            false,
        ),
        (
            "Or:\n          - Variable: $.n\n            NumericEquals: 4\n          - Variable: $.f\n            NumericEquals: 2.50",
            true,
        ),
    ];
    for (rule, expected) in cases {
        assert_eq!(decides(rule, input.clone()), expected, "{rule}");
    }
}

#[test]
fn a_missing_variable_is_a_runtime_error() {
    let sim = run_skill(
        &choice_skill("Variable: $.missing\n        StringEquals: x"),
        json!({}),
        echo(),
    );
    let run = sim.run_record(1).run;
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(run.error.as_deref(), Some("States.Runtime"));
}

#[test]
fn no_choice_matched() {
    let yaml = "Inputs: {}
StartAt: Decide
TimeoutSeconds: 60
States:
  Decide:
    Type: Choice
    Choices:
      - Variable: $.n
        NumericEquals: 1
        Next: Done
  Done:
    Type: Succeed
";
    let sim = run_skill(yaml, json!({"n": 2}), echo());
    let run = sim.run_record(1).run;
    assert_eq!(run.error.as_deref(), Some("States.NoChoiceMatched"));
    assert_eq!(sim.entries(1)[0].status, EntryStatus::Failed);
}

#[test]
fn runtime_errors_pass_every_retrier_and_catcher() {
    let yaml = r#"Inputs: {}
StartAt: Run
TimeoutSeconds: 60
States:
  Run:
    Type: Command
    Run.$: "States.Format('echo {}', $.missing)"
    TimeoutSeconds: 10
    Retry:
      - ErrorEquals: [States.ALL]
        IntervalSeconds: 1
        MaxAttempts: 3
        BackoffRate: 1
    Catch:
      - ErrorEquals: [States.ALL]
        Next: Caught
    End: true
  Caught:
    Type: Succeed
"#;
    let sim = run_skill(yaml, json!({}), echo());
    let run = sim.run_record(1).run;
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(run.error.as_deref(), Some("States.Runtime"));
    assert!(sim.calls.is_empty());
    assert_eq!(
        sim.notices(),
        [
            "[sanchoris] Skill test-skill (run 1): Run -> FAILED.\nError: States.Runtime\nCause: a path names no value"
        ]
    );
}

#[test]
fn a_bad_result_path_is_a_runtime_error() {
    let yaml = "Inputs: {}
StartAt: Run
TimeoutSeconds: 60
States:
  Run:
    Type: Command
    Run: echo hello
    TimeoutSeconds: 10
    ResultPath: $.list.item
    Catch:
      - ErrorEquals: [States.ALL]
        Next: Caught
    End: true
  Caught:
    Type: Succeed
";
    let sim = run_skill(yaml, json!({"list": [1]}), echo());
    let run = sim.run_record(1).run;
    assert_eq!(run.error.as_deref(), Some("States.Runtime"));
    assert_eq!(sim.entries(1).len(), 1);
}

#[test]
fn the_catch_result_has_the_error_and_the_output() {
    let yaml = "Inputs: {}
StartAt: Run
TimeoutSeconds: 60
States:
  Run:
    Type: Command
    Run: make test
    TimeoutSeconds: 10
    Catch:
      - ErrorEquals: [Command.Failed]
        ResultPath: $.failure
        Next: Discard
    End: true
  Discard:
    Type: Command
    Run: 'false'
    TimeoutSeconds: 10
    Catch:
      - ErrorEquals: [States.ALL]
        ResultPath: null
        Next: Done
    End: true
  Done:
    Type: Succeed
";
    let sim = run_skill(
        yaml,
        json!({"keep": 1}),
        Box::new(|call: &Call| fails(10, 2, &format!("{}\n", call.state), "boom\n")),
    );
    assert_eq!(sim.run_record(1).run.status, RunStatus::Succeeded);
    assert_eq!(
        output(&sim),
        json!({
            "keep": 1,
            "failure": {
                "Error": "Command.Failed",
                "Cause": "the command exited with code 2",
                "ExitCode": 2,
                "Stdout": "Run\n",
                "Stderr": "boom\n"
            }
        })
    );
    let statuses: Vec<_> = sim.entries(1).iter().map(|e| e.status).collect();
    assert_eq!(
        statuses,
        [
            EntryStatus::Caught,
            EntryStatus::Caught,
            EntryStatus::Succeeded
        ]
    );
}

#[test]
fn a_timeout_that_is_caught_stays_timed_out() {
    let yaml = "Inputs: {}
StartAt: Run
TimeoutSeconds: 60
States:
  Run:
    Type: Command
    Run: sleep 100
    TimeoutSeconds: 5
    Catch:
      - ErrorEquals: [States.Timeout]
        Next: Done
    End: true
  Done:
    Type: Succeed
";
    let sim = run_skill(yaml, json!({}), Box::new(|_: &Call| Behavior::TimesOut));
    assert_eq!(sim.run_record(1).run.status, RunStatus::Succeeded);
    assert_eq!(sim.entries(1)[0].status, EntryStatus::TimedOut);
    assert_eq!(output(&sim)["Error"], "States.Timeout");
}

#[test]
fn wait_seconds_and_timestamp() {
    let yaml = "Inputs: {}
StartAt: Pause
TimeoutSeconds: 86400
States:
  Pause:
    Type: Wait
    Seconds: 90
    Next: Until
  Until:
    Type: Wait
    Timestamp: '2026-11-28T10:00:00+09:00'
    Next: Done
  Done:
    Type: Succeed
";
    let mut sim = Sim::new(
        skills(&[("test-skill", yaml)]),
        echo(),
        Box::new(|_, _| Vec::new()),
    );
    // 2026-11-28T00:00:00Z.
    sim.now = 1_795_824_000_000;
    let start = sim.now;
    sim.start("test-skill", json!({"a": 1}));
    sim.run_until(start + 1_000);
    assert_eq!(sim.run_record(1).run.status, RunStatus::Waiting);
    assert_eq!(sim.entries(1)[0].status, EntryStatus::Waiting);
    sim.run();
    let entries = sim.entries(1);
    assert_eq!(u64_of(&entries[1].entered_at) - start, 90_000);
    assert_eq!(u64_of(&entries[2].entered_at) - start, 3_600_000);
    assert_eq!(output(&sim), json!({"a": 1}));
}

#[test]
fn hidden_transitions_go_with_the_next_notification() {
    let yaml = "Inputs: {}
StartAt: A
TimeoutSeconds: 60
States:
  A:
    Type: Command
    Run: echo a
    TimeoutSeconds: 10
    Prompt: { Transition: false }
    Next: B
  B:
    Type: Pass
    Prompt: { Transition: false }
    Next: C
  C:
    Type: Command
    Run: echo c
    TimeoutSeconds: 10
    Prompt: { Stdout: false }
    Next: D
  D:
    Type: Command
    Run: echo d
    TimeoutSeconds: 10
    Prompt: { Transition: false }
    End: true
";
    let sim = run_skill(yaml, json!({}), echo());
    let notices = sim.notices();
    assert_eq!(notices.len(), 2);
    assert_eq!(
        notices[0],
        "[sanchoris] Skill test-skill (run 1): A -> B -> C -> D. Exit 0."
    );
    assert!(
        notices[1].starts_with("[sanchoris] Skill test-skill (run 1): D -> SUCCEEDED.\nOutput: ")
    );
}

#[test]
fn the_notification_keeps_the_end_of_long_output() {
    let yaml = "Inputs: {}
StartAt: Log
TimeoutSeconds: 60
States:
  Log:
    Type: Command
    Run: cat build.log
    TimeoutSeconds: 10
    Next: Done
  Done:
    Type: Succeed
";
    let long = format!("{}END", "x".repeat(OUTPUT_TAIL_BYTES * 2));
    let sim = run_skill(
        yaml,
        json!({}),
        Box::new(move |_: &Call| fails(10, 0, &long, &long)),
    );
    let first = &sim.notices()[0];
    let stdout = first
        .split("\nstdout: ")
        .nth(1)
        .unwrap()
        .split("\nstderr: ")
        .next()
        .unwrap();
    assert_eq!(stdout.len(), OUTPUT_TAIL_BYTES);
    assert!(stdout.ends_with("END"));
    // The StateEntry keeps the whole output.
    let result = sim.entries(1)[0].result.clone().unwrap();
    assert_eq!(result.stdout.len(), OUTPUT_TAIL_BYTES * 2 + 3);
}

#[test]
fn abort_closes_the_open_entry() {
    let yaml = "Inputs: {}
StartAt: Run
TimeoutSeconds: 60
States:
  Run:
    Type: Command
    Run: sleep 30
    TimeoutSeconds: 40
    End: true
";
    let mut sim = Sim::new(
        skills(&[("test-skill", yaml)]),
        Box::new(|_: &Call| Behavior::Hang),
        Box::new(|_, _| Vec::new()),
    );
    sim.start("test-skill", json!({}));
    sim.run_until(sim.now + 1_000);
    let now = sim.now;
    sim.engine.abort(1, now).unwrap();
    let run = sim.run_record(1).run;
    assert_eq!(run.status, RunStatus::Aborted);
    assert_eq!(sim.entries(1)[0].status, EntryStatus::Aborted);
    assert!(
        sim.notices().is_empty(),
        "ABORTED is not sent to the thread"
    );
    // The timers of the aborted SkillRun do nothing.
    let commits = sim.engine.store.commits;
    sim.run();
    assert_eq!(sim.engine.store.commits, commits);
}
