//! The parent of a SkillRun (`skill.md`「Skill の入れ子」).

use crate::common::*;
use sanchoris_engine::json::nat;
use sanchoris_engine::lean::{Parent, RunStatus};
use sanchoris_engine::{Origin, StartRequest};
use serde_json::json;

/// An outer loop: `Implement` waits for the thread, and the loop ends when the Output says
/// `Done`.
const ISSUE_LOOP: &str = "Inputs: {}
StartAt: Implement
TimeoutSeconds: 86400
States:
  Implement:
    Type: Command
    Run: sanchoris-agent skill await
    TimeoutSeconds: 3600
    ResultPath: $.implement
    Next: Check
  Check:
    Type: Choice
    Choices:
      - Variable: $.implement.Json.Done
        BooleanEquals: true
        Next: Finish
    Default: Implement
  Finish:
    Type: Succeed
";

/// A command that runs until the test ends.
const WAIT_PR_CI: &str = "Inputs: {}
StartAt: Watch
TimeoutSeconds: 86400
States:
  Watch:
    Type: Command
    Run: gh pr checks --watch
    TimeoutSeconds: 3600
    End: true
";

async fn sim() -> Sim {
    Sim::new(
        skills(&[("issue-loop", ISSUE_LOOP), ("wait-pr-ci", WAIT_PR_CI)]),
        Box::new(|call: &Call| match call.state.as_str() {
            "Implement" => Behavior::Await { after_ms: 0 },
            _ => Behavior::Hang,
        }),
        Box::new(|_, _| Vec::new()),
    )
    .await
}

fn parent(run: u64, state: &str, visit: u64) -> Option<Parent> {
    Some(Parent {
        run: nat(run),
        state: state.to_string(),
        visit: nat(visit),
    })
}

async fn step(sim: &mut Sim) {
    let until = sim.now + 1_000;
    sim.run_until(until).await;
    sim.now = until;
}

#[tokio::test]
async fn a_skill_with_no_await_in_the_thread_is_under_the_thread() {
    let mut sim = sim().await;
    sim.start("wait-pr-ci", json!({}));
    step(&mut sim).await;
    assert_eq!(sim.run_record(1).await.parent, None);
    // A SkillRun that runs a command but does not wait in await is no parent.
    sim.start("wait-pr-ci", json!({}));
    step(&mut sim).await;
    assert_eq!(sim.run_record(2).await.parent, None);
}

#[tokio::test]
async fn a_skill_called_during_await_is_a_child_of_that_entry() {
    let mut sim = sim().await;
    sim.start("issue-loop", json!({}));
    step(&mut sim).await;
    sim.start("wait-pr-ci", json!({}));
    step(&mut sim).await;
    assert_eq!(sim.run_record(2).await.parent, parent(1, "Implement", 1));
    // The second time round the loop, the child is under the second entry into Implement.
    sim.continue_(0, 1, json!({"Done": false}));
    step(&mut sim).await;
    assert_eq!(
        sim.run_record(1).await.run.current.as_ref().unwrap().visit,
        nat(2)
    );
    sim.start("wait-pr-ci", json!({}));
    step(&mut sim).await;
    assert_eq!(sim.run_record(3).await.parent, parent(1, "Implement", 2));
    // The children go on after the parent leaves the State.
    assert_eq!(sim.run_record(2).await.run.status, RunStatus::Running);
    sim.continue_(0, 1, json!({"Done": true}));
    step(&mut sim).await;
    assert_eq!(sim.run_record(1).await.run.status, RunStatus::Succeeded);
    assert_eq!(sim.run_record(2).await.run.status, RunStatus::Running);
    assert_eq!(sim.run_record(3).await.run.status, RunStatus::Running);
    sim.start("wait-pr-ci", json!({}));
    step(&mut sim).await;
    assert_eq!(sim.run_record(4).await.parent, None);
}

#[tokio::test]
async fn the_await_entered_last_is_the_parent() {
    let mut sim = sim().await;
    sim.start("issue-loop", json!({}));
    step(&mut sim).await;
    // The same skill inside itself: run 2 is a child of run 1 and waits in await after it.
    sim.start("issue-loop", json!({}));
    step(&mut sim).await;
    assert_eq!(sim.run_record(2).await.parent, parent(1, "Implement", 1));
    sim.start("wait-pr-ci", json!({}));
    step(&mut sim).await;
    assert_eq!(sim.run_record(3).await.parent, parent(2, "Implement", 1));
    // When run 2 leaves its await, run 1 is the one that waits.
    sim.continue_(0, 2, json!({"Done": true}));
    step(&mut sim).await;
    assert_eq!(sim.run_record(2).await.run.status, RunStatus::Succeeded);
    sim.start("wait-pr-ci", json!({}));
    step(&mut sim).await;
    assert_eq!(sim.run_record(4).await.parent, parent(1, "Implement", 1));
    // Run 1 enters await again, after run 2 did: it is still the last one.
    sim.continue_(0, 1, json!({"Done": false}));
    step(&mut sim).await;
    sim.start("wait-pr-ci", json!({}));
    step(&mut sim).await;
    assert_eq!(sim.run_record(5).await.parent, parent(1, "Implement", 2));
}

#[tokio::test]
async fn a_skill_the_user_starts_follows_the_same_rule() {
    let mut sim = sim().await;
    sim.start("issue-loop", json!({}));
    step(&mut sim).await;
    let skill = sim.skills["wait-pr-ci"].clone();
    let now = sim.now;
    sim.engine
        .start(
            StartRequest {
                request: "user-1".to_string(),
                thread: THREAD.to_string(),
                origin: Origin::User,
                definition: skill.definition,
                source: skill.source,
                input: json!({}),
            },
            now,
        )
        .await
        .unwrap();
    let record = sim.run_record(2).await;
    assert_eq!(record.origin, Origin::User);
    assert_eq!(record.parent, parent(1, "Implement", 1));
}

#[tokio::test]
async fn skill_runs_of_other_threads_are_no_parents() {
    let mut sim = sim().await;
    sim.start("issue-loop", json!({}));
    step(&mut sim).await;
    let skill = sim.skills["wait-pr-ci"].clone();
    let now = sim.now;
    sim.engine
        .start(
            StartRequest {
                request: "other".to_string(),
                thread: "th_other".to_string(),
                origin: Origin::Thread,
                definition: skill.definition,
                source: skill.source,
                input: json!({}),
            },
            now,
        )
        .await
        .unwrap();
    assert_eq!(sim.run_record(2).await.parent, None);
}
