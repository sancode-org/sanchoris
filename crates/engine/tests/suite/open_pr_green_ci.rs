//! `open-pr-green-ci` against a Workspace whose commands answer from a script.

use crate::common::*;
use sanchoris_engine::lean::{EntryStatus, RunStatus};
use sanchoris_engine::{ExecKey, Response, exec_output};
use serde_json::{Value, json};

/// How the commands of `open-pr-green-ci` answer.
#[derive(Debug, Clone, Copy)]
struct Plan {
    /// The attempts of the first `Push` that fail.
    push_failures: u64,
    /// The first entry into `WaitChecks` whose checks pass; `None` when they never pass.
    checks_pass_at: Option<u64>,
    /// Whether `WaitChecks` hangs instead of answering.
    checks_hang: bool,
    /// How long after the command starts `sanchoris-agent skill await` asks the engine.
    await_after_ms: u64,
    /// How long after the notification of `AwaitFix` the agent runs `continue`.
    continue_after_ms: u64,
}

const PLAN: Plan = Plan {
    push_failures: 0,
    checks_pass_at: Some(1),
    checks_hang: false,
    await_after_ms: 500,
    continue_after_ms: 5_000,
};

const CHECKS_STDOUT: &str = "checkout_spec::flaky ... fail\n";

fn script(plan: Plan) -> Script {
    Box::new(move |call: &Call| match call.state.as_str() {
        "Push" => {
            if call.visit == 1 && call.attempt <= plan.push_failures {
                fails(500, 1, "", "! [rejected] HEAD -> sanchoris/th_01JD8Q2M4K\n")
            } else {
                ok(1_000, "")
            }
        }
        "OpenPR" => ok(2_000, &json_out(&pr())),
        "WaitChecks" => {
            if plan.checks_hang {
                Behavior::Hang
            } else if plan.checks_pass_at.is_some_and(|v| call.visit >= v) {
                ok(60_000, "all checks passed\n")
            } else {
                fails(
                    60_000,
                    1,
                    CHECKS_STDOUT,
                    "some checks were not successful\n",
                )
            }
        }
        "AskFix" => {
            let url = call.input["pr"]["Json"]["url"].as_str().unwrap();
            let checks = call.input["checks"]["Stdout"].as_str().unwrap();
            let run = call.request.key.run;
            ok(
                100,
                &format!(
                    "CI failed on {url}. Fix it, commit, and run: sanchoris-agent skill continue {run}\n{checks}\n"
                ),
            )
        }
        "AwaitFix" => Behavior::Await {
            after_ms: plan.await_after_ms,
        },
        "Report" => ok(1_000, ""),
        other => panic!("no command in {other}"),
    })
}

fn agent(plan: Plan) -> Agent {
    Box::new(move |run, text| {
        if text.contains("-> AwaitFix.") {
            vec![Action::Continue {
                after_ms: plan.continue_after_ms,
                run,
                output: json!({"Fixed": true}),
            }]
        } else {
            Vec::new()
        }
    })
}

async fn sim(plan: Plan) -> Sim {
    let mut sim = Sim::new(skills(&[]), script(plan), agent(plan)).await;
    sim.start("open-pr-green-ci", json!({"Repo": "frontend", "Issue": 35}));
    sim
}

async fn states(sim: &mut Sim) -> Vec<String> {
    sim.entries(1)
        .await
        .iter()
        .map(|e| e.state.clone())
        .collect()
}

const HEAD: &str = "[sanchoris] Skill open-pr-green-ci (run 1): ";

fn notice(text: &str) -> String {
    format!("{HEAD}{text}")
}

#[tokio::test]
async fn the_checks_pass() {
    let mut sim = sim(PLAN).await;
    sim.run().await;
    let record = sim.run_record(1).await;
    assert_eq!(record.run.status, RunStatus::Succeeded);
    assert_eq!(
        states(&mut sim).await,
        ["Init", "Push", "OpenPR", "WaitChecks", "Report"]
    );
    let output = sanchoris_engine::json::from_lean(record.run.output.as_ref().unwrap());
    assert_eq!(
        output,
        json!({
            "Repo": "frontend",
            "Issue": 35,
            "fixes": {"Count": 0},
            "pr": {"ExitCode": 0, "Stdout": json_out(&pr()), "Stderr": "", "Json": pr()}
        })
    );
    assert_eq!(
        sim.notices().await,
        [
            notice("Init -> Push -> OpenPR. Exit 0."),
            notice(&format!("OpenPR -> WaitChecks. Exit 0.\nstdout: {}", pr())),
            notice("WaitChecks -> Report. Exit 0."),
            notice(&format!("Report -> SUCCEEDED.\nOutput: {output}")),
        ]
    );
    let push = &sim.calls[0];
    assert_eq!(
        push.request.command,
        "cd frontend && git push -u origin HEAD"
    );
    assert_eq!(
        push.request.key,
        ExecKey {
            run: 1,
            seq: 2,
            attempt: 1
        }
    );
    assert_eq!(push.request.key.to_string(), "1:2:1");
    assert_eq!(push.request.timeout_seconds, 300);
    assert_eq!(
        push.input,
        json!({"Repo": "frontend", "Issue": 35, "fixes": {"Count": 0}})
    );
    let report = sim.calls.last().unwrap();
    assert_eq!(report.state, "Report");
    assert_eq!(report.input["pr"]["Json"]["url"], pr()["url"]);
    assert_eq!(
        sim.responses[0].1,
        Response::Ok(json!({"RunId": 1, "Status": "RUNNING"}))
    );
}

#[tokio::test]
async fn a_failure_of_ci_goes_to_the_agent_and_its_fix_passes() {
    let mut sim = sim(Plan {
        checks_pass_at: Some(2),
        ..PLAN
    })
    .await;
    sim.run().await;
    assert_eq!(sim.run_record(1).await.run.status, RunStatus::Succeeded);
    assert_eq!(
        states(&mut sim).await,
        [
            "Init",
            "Push",
            "OpenPR",
            "WaitChecks",
            "CanFix",
            "AskFix",
            "AwaitFix",
            "CountFix",
            "Push",
            "OpenPR",
            "WaitChecks",
            "Report"
        ]
    );
    let ask = sim.calls.iter().find(|c| c.state == "AskFix").unwrap();
    assert_eq!(ask.input["checks"]["Error"], "Command.Failed");
    assert_eq!(ask.input["checks"]["ExitCode"], 1);
    assert_eq!(ask.input["checks"]["Stdout"], CHECKS_STDOUT);
    let entries = sim.entries(1).await;
    assert_eq!(entries[3].status, EntryStatus::Caught);
    let second_push = sim
        .calls
        .iter()
        .filter(|c| c.state == "Push")
        .nth(1)
        .unwrap();
    assert_eq!(second_push.input["fixes"], json!({"Count": 1}));
    assert_eq!(second_push.visit, 2);
    let notices = sim.notices().await;
    assert_eq!(
        notices[2],
        notice("WaitChecks -> CanFix. Exit 1.\nstderr: some checks were not successful")
    );
    assert_eq!(
        notices[3],
        notice(&format!(
            "CanFix -> AskFix -> AwaitFix. Exit 0.\nstdout: CI failed on https://github.com/acme/shop-web/pull/52. Fix it, commit, and run: sanchoris-agent skill continue 1\n{}",
            CHECKS_STDOUT.trim_end()
        ))
    );
    assert_eq!(
        notices[4],
        notice("AwaitFix -> CountFix. Exit 0.\nstdout: {\"Fixed\":true}")
    );
    assert_eq!(notices[5], notice("CountFix -> Push -> OpenPR. Exit 0."));
}

#[tokio::test]
async fn the_fixes_run_out() {
    let mut sim = sim(Plan {
        checks_pass_at: None,
        ..PLAN
    })
    .await;
    sim.run().await;
    let record = sim.run_record(1).await;
    assert_eq!(record.run.status, RunStatus::Failed);
    assert_eq!(record.run.error.as_deref(), Some("Skill.FixesExhausted"));
    let asks = sim.calls.iter().filter(|c| c.state == "AskFix").count();
    assert_eq!(asks, 3);
    let checks = sim.calls.iter().filter(|c| c.state == "WaitChecks").count();
    assert_eq!(checks, 4);
    assert_eq!(
        sim.notices().await.last().unwrap(),
        &notice(
            "CanFix -> GiveUp -> FAILED.\nError: Skill.FixesExhausted\nCause: The checks still fail after 3 fixes"
        )
    );
    assert_eq!(sim.entries(1).await.last().unwrap().state, "GiveUp");
}

#[tokio::test]
async fn push_is_retried_with_backoff() {
    let mut sim = sim(Plan {
        push_failures: 2,
        ..PLAN
    })
    .await;
    let start = sim.now;
    sim.run().await;
    assert_eq!(sim.run_record(1).await.run.status, RunStatus::Succeeded);
    let pushes: Vec<_> = sim.calls.iter().filter(|c| c.state == "Push").collect();
    assert_eq!(
        pushes.iter().map(|c| c.attempt).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    let push = &sim.entries(1).await[1];
    assert_eq!(push.state, "Push");
    assert_eq!(push.status, EntryStatus::Succeeded);
    assert_eq!(push.attempts.to_string(), "3");
    assert_eq!(
        push.retry_counts
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>(),
        ["2"]
    );
    // The first attempt fails after 0.5 s and waits 10 s, the second waits 20 s.
    let opened: Vec<u64> = sim
        .engine
        .store
        .log
        .iter()
        .flat_map(|tx| tx.jobs.iter())
        .filter_map(|j| match j {
            sanchoris_engine::Job::Timer {
                timer: sanchoris_engine::lean::Timer::Retry { .. },
                due,
                ..
            } => Some(*due - start),
            _ => None,
        })
        .collect();
    assert_eq!(opened, [10_500, 31_000]);
    assert_eq!(
        sim.notices().await[0],
        notice("Init -> Push -> OpenPR. Exit 0.")
    );
}

#[tokio::test]
async fn push_fails_when_its_retries_run_out() {
    let mut sim = sim(Plan {
        push_failures: 4,
        ..PLAN
    })
    .await;
    sim.run().await;
    let record = sim.run_record(1).await;
    assert_eq!(record.run.status, RunStatus::Failed);
    assert_eq!(record.run.error.as_deref(), Some("Command.Failed"));
    assert_eq!(sim.calls.iter().filter(|c| c.state == "Push").count(), 4);
    assert_eq!(sim.entries(1).await[1].status, EntryStatus::Failed);
    assert_eq!(
        sim.notices().await.last().unwrap(),
        &notice(
            "Init -> Push -> FAILED.\nError: Command.Failed\nCause: the command exited with code 1"
        )
    );
}

#[tokio::test]
async fn a_command_that_hangs_times_out() {
    let mut sim = sim(Plan {
        checks_hang: true,
        ..PLAN
    })
    .await;
    let start = sim.now;
    sim.run().await;
    let record = sim.run_record(1).await;
    assert_eq!(record.run.status, RunStatus::Failed);
    assert_eq!(record.run.error.as_deref(), Some("States.Timeout"));
    let checks = &sim.entries(1).await[3];
    assert_eq!(checks.state, "WaitChecks");
    assert_eq!(checks.status, EntryStatus::TimedOut);
    let ended = sanchoris_engine::json::u64_of(record.run.finished_at.as_ref().unwrap());
    // Push 1 s, OpenPR 2 s, then the 3600 s of WaitChecks.
    assert_eq!(ended - start, 3_000 + 3_600_000);
}

#[tokio::test]
async fn a_timed_out_result_and_the_timer_end_the_state_once() {
    let mut plan = PLAN;
    plan.checks_hang = true;
    let mut sim = sim(plan).await;
    // The Workspace kills the command at its TimeoutSeconds and answers timedOut, at the same
    // time as the timer of the engine.
    sim.run_until(sim.now + 3_000 + 3_600_000 - 1).await;
    let key = ExecKey {
        run: 1,
        seq: 4,
        attempt: 1,
    };
    sim.engine
        .exec_result(key, exec_output(-1, "", "", true), sim.now)
        .await
        .unwrap();
    let commits = sim.engine.store.commits;
    sim.run().await;
    assert_eq!(sim.engine.store.commits, commits, "the timer does nothing");
    let record = sim.run_record(1).await;
    assert_eq!(record.run.error.as_deref(), Some("States.Timeout"));
    assert_eq!(sim.entries(1).await[3].status, EntryStatus::TimedOut);
}

#[tokio::test]
async fn the_skill_run_times_out() {
    // Each check takes an hour less a second and fails, and each fix 10 s: the fourth check
    // runs at the end of the 4 hours.
    let mut sim = Sim::new(
        skills(&[]),
        Box::new(|call: &Call| match call.state.as_str() {
            "WaitChecks" => fails(3_599_000, 1, CHECKS_STDOUT, ""),
            "AwaitFix" => Behavior::Await { after_ms: 0 },
            "OpenPR" => ok(0, &json_out(&pr())),
            _ => ok(0, &format!("{}\n", call.state)),
        }),
        agent(Plan {
            continue_after_ms: 10_000,
            ..PLAN
        }),
    )
    .await;
    let start = sim.now;
    sim.start("open-pr-green-ci", json!({"Repo": "frontend", "Issue": 35}));
    sim.run().await;
    let record = sim.run_record(1).await;
    assert_eq!(record.run.status, RunStatus::Failed);
    assert_eq!(record.run.error.as_deref(), Some("States.Timeout"));
    assert_eq!(
        sanchoris_engine::json::u64_of(record.run.finished_at.as_ref().unwrap()) - start,
        14_400_000
    );
    let last = sim.entries(1).await.last().unwrap().clone();
    assert_eq!(last.status, EntryStatus::Failed);
    assert_eq!(last.error.as_deref(), Some("States.Timeout"));
}

#[tokio::test]
async fn continue_before_await_is_kept_for_it() {
    let mut sim = sim(Plan {
        checks_pass_at: Some(2),
        await_after_ms: 10_000,
        continue_after_ms: 0,
        ..PLAN
    })
    .await;
    sim.run().await;
    assert_eq!(sim.run_record(1).await.run.status, RunStatus::Succeeded);
    let await_fix = sim
        .entries(1)
        .await
        .into_iter()
        .find(|e| e.state == "AwaitFix")
        .unwrap();
    assert_eq!(
        await_fix.r#await,
        sanchoris_engine::lean::Await::Delivered {
            output: sanchoris_engine::json::to_lean(&json!({"Fixed": true}))
        }
    );
    assert!(sim.run_record(1).await.run.pending_continue.is_none());
}

#[tokio::test]
async fn continue_answers_errors() {
    let mut sim = sim(Plan {
        checks_pass_at: None,
        await_after_ms: 3_600_000,
        continue_after_ms: 0,
        ..PLAN
    })
    .await;
    // The first continue waits for the await; the second one fails.
    sim.run_until(sim.now + 125_000).await;
    let now = sim.now;
    let second = sim
        .engine
        .continue_("again", 1, &json!({}), now)
        .await
        .unwrap();
    assert_eq!(
        second,
        Response::Error {
            code: "Skill.ContinuePending".to_string(),
            message: "SkillRun 1 already has a continue waiting".to_string()
        }
    );
    assert_eq!(
        sim.engine
            .continue_("again", 1, &json!({}), now)
            .await
            .unwrap(),
        second,
        "a repeated request has the same answer"
    );
    sim.run().await;
    let now = sim.now;
    assert_eq!(
        sim.engine
            .continue_("late", 1, &json!({}), now)
            .await
            .unwrap(),
        Response::Error {
            code: "Skill.Finished".to_string(),
            message: "SkillRun 1 has finished".to_string()
        }
    );
}

#[tokio::test]
async fn a_restored_await_returns_the_output_it_had() {
    let mut sim = sim(Plan {
        checks_pass_at: Some(2),
        ..PLAN
    })
    .await;
    // Run until the await has its Output but its command has not answered.
    sim.await_answer_after_ms = 1_000;
    sim.run_until(sim.now + 3_000 + 60_000 + 100 + 5_000).await;
    let record = sim.run_record(1).await;
    let e = record.run.current.clone().unwrap();
    assert_eq!(e.state, "AwaitFix");
    assert!(matches!(
        e.r#await,
        sanchoris_engine::lean::Await::Delivered { .. }
    ));
    let before = sim.outbox().await.len();
    let now = sim.now;
    let again = sim.engine.await_("restored", 1, now).await.unwrap();
    assert!(matches!(again, Response::Ok(_)));
    let after = &sim.outbox().await[before..];
    assert_eq!(
        after,
        [sanchoris_engine::Message::ContinueOutput {
            run: 1,
            output: json!({"Fixed": true})
        }]
    );
}

#[tokio::test]
async fn a_result_with_the_same_key_is_used_again() {
    let mut sim = sim(PLAN).await;
    // Stop the store right after the result of Push is written, before its transition.
    sim.run_until(sim.now + 999).await;
    let key = ExecKey {
        run: 1,
        seq: 2,
        attempt: 1,
    };
    let ops = sim.engine.store.ops;
    sim.engine.store.crash_at.insert(ops + 2);
    let r = sim
        .engine
        .exec_result(key, exec_output(0, "", "", false), sim.now + 1)
        .await;
    assert!(r.is_err());
    assert!(sim.has_exec_result(key).await);
    assert_eq!(
        sim.run_record(1).await.run.current.as_ref().unwrap().state,
        "Push"
    );
    // A restored Workspace would run the command again; the engine takes the result instead.
    sim.now += 1;
    sim.restore_workspace().await;
    assert_eq!(sim.sent[&key], 1, "the command is not sent again");
    assert_eq!(
        sim.run_record(1).await.run.current.as_ref().unwrap().state,
        "OpenPR"
    );
    sim.run().await;
    assert_eq!(sim.run_record(1).await.run.status, RunStatus::Succeeded);
    assert_eq!(sim.calls.iter().filter(|c| c.state == "Push").count(), 1);
}

#[tokio::test]
async fn each_transition_is_one_write() {
    let mut sim = sim(Plan {
        checks_pass_at: Some(2),
        ..PLAN
    })
    .await;
    sim.run().await;
    let log = &sim.engine.store.log;
    // Every transaction writes its SkillRun over the version it read.
    for (i, tx) in log.iter().enumerate() {
        assert_eq!(tx.run.version, i as u64);
        assert_eq!(tx.expected_version, i.checked_sub(1).map(|v| v as u64));
    }
    // A transaction that closes a StateEntry and opens the next writes both, with the
    // notification and the command of the next one.
    let ask_to_await = log
        .iter()
        .find(|tx| tx.entries.iter().any(|e| e.state == "AwaitFix"))
        .unwrap();
    let names: Vec<&str> = ask_to_await
        .entries
        .iter()
        .map(|e| e.state.as_str())
        .collect();
    assert_eq!(names, ["AskFix", "AwaitFix"]);
    assert_eq!(ask_to_await.messages.len(), 1);
    assert_eq!(
        ask_to_await
            .jobs
            .iter()
            .filter(|j| matches!(j, sanchoris_engine::Job::Exec(_)))
            .count(),
        1
    );
    // Every write of the store besides the results and the deletion of jobs is a commit.
    assert_eq!(sim.engine.store.commits, log.len());
}

async fn snapshot(sim: &mut Sim) -> (Vec<(String, String)>, Value, Vec<String>, RunStatus) {
    let record = sim.run_record(1).await;
    (
        sim.path(1).await,
        record
            .run
            .output
            .as_ref()
            .map(sanchoris_engine::json::from_lean)
            .unwrap_or(Value::Null),
        sim.notices().await,
        record.run.status,
    )
}

/// Stops the store at each of its operations in turn, before the operation and after it, and
/// goes on from what it holds: the SkillRun, its StateEntries, its notifications and the
/// commands the Workspace ran end the same as without the stop. The stopped runs go 8 at a
/// time, each on its own thread and runtime.
async fn sweep(plan: Plan) {
    let mut baseline = sim(plan).await;
    baseline.run().await;
    let expected = snapshot(&mut baseline).await;
    let entries = baseline.all_entries().await;
    let calls = baseline.calls.len();
    let total = baseline.engine.store.ops;
    let stops: Vec<(usize, bool)> = (1..=total)
        .flat_map(|op| [(op, false), (op, true)])
        .collect();
    for chunk in stops.chunks(8) {
        let runs: Vec<_> = chunk
            .iter()
            .map(|&(op, after)| {
                std::thread::spawn(move || {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("a runtime");
                    runtime.block_on(async move {
                        let mut s = sim(plan).await;
                        if after {
                            s.engine.store.crash_after.insert(op);
                        } else {
                            s.engine.store.crash_at.insert(op);
                        }
                        s.run().await;
                        (
                            s.crashes,
                            snapshot(&mut s).await,
                            s.all_entries().await,
                            s.calls.len(),
                        )
                    })
                })
            })
            .collect();
        for (&(op, after), run) in chunk.iter().zip(runs) {
            let (crashes, snap, all, n) = run.join().expect("a stopped run");
            let at = format!("a stop at operation {op} (after it: {after})");
            if !after {
                assert_eq!(crashes, 1, "{at}");
            }
            assert_eq!(snap, expected, "{at}");
            assert_eq!(all, entries, "{at}");
            assert_eq!(n, calls, "{at}");
        }
    }
}

#[tokio::test]
async fn a_stop_at_any_operation_ends_the_same() {
    sweep(Plan {
        checks_pass_at: Some(2),
        ..PLAN
    })
    .await;
}

#[tokio::test]
async fn a_stop_at_any_operation_ends_the_same_with_retries_and_continue_before_await() {
    sweep(Plan {
        push_failures: 1,
        checks_pass_at: Some(2),
        await_after_ms: 10_000,
        continue_after_ms: 0,
        ..PLAN
    })
    .await;
}
