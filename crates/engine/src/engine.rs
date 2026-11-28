//! The engine-worker: takes the events of SkillRuns, runs the Lean transition, and writes each
//! transition as one transaction.

use serde_json::{Value, json};

use crate::generated::sanchoris_engine::{
    self as lean, Definition, Event, ExecOutput, Outcome, Phase, Rejection, Run, RunStatus, Write,
};
use crate::json::{from_lean, nat, to_lean, u64_of};
use crate::notice;
use crate::store::{
    ExecKey, ExecRequest, Job, JobRecord, Message, Origin, Response, RunRecord, Store, StoreError,
    Transaction,
};

/// The Workspace of a thread, as the engine sees it: it runs a command and answers later with
/// [`Engine::exec_result`].
pub trait Workspace: Send {
    fn exec(&mut self, request: &ExecRequest) -> impl Future<Output = ()> + Send;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("no SkillRun {0}")]
    UnknownRun(u64),
}

/// What an event did to its SkillRun.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    Applied,
    Ignored,
    Rejected(Rejection),
}

/// A request to start a SkillRun in a thread.
#[derive(Debug, Clone, PartialEq)]
pub struct StartRequest {
    /// The ID of the request, which answers a repeated request the same way.
    pub request: String,
    pub thread: String,
    pub origin: Origin,
    pub definition: Definition,
    pub input: Value,
}

pub fn status_name(status: &RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "RUNNING",
        RunStatus::Waiting => "WAITING",
        RunStatus::Succeeded => "SUCCEEDED",
        RunStatus::Failed => "FAILED",
        RunStatus::Aborted => "ABORTED",
    }
}

fn run_response(run: &Run) -> Response {
    Response::Ok(json!({ "RunId": u64_of(&run.id), "Status": status_name(&run.status) }))
}

fn rejection_response(run: u64, reason: &Rejection) -> Response {
    let (code, message) = match reason {
        Rejection::Finished => ("Skill.Finished", format!("SkillRun {run} has finished")),
        Rejection::ContinuePending => (
            "Skill.ContinuePending",
            format!("SkillRun {run} already has a continue waiting"),
        ),
        Rejection::NotAwaiting => (
            "Skill.NotAwaiting",
            format!("SkillRun {run} does not run a command"),
        ),
    };
    Response::Error {
        code: code.to_string(),
        message,
    }
}

/// The result of a command as the Lean engine takes it.
pub fn exec_output(exit_code: i64, stdout: &str, stderr: &str, timed_out: bool) -> ExecOutput {
    ExecOutput {
        exit_code: leanfran::Int::from(exit_code),
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
        timed_out,
    }
}

pub struct Engine<S> {
    pub store: S,
}

/// The event of a transition: one that is known, or the start of `sanchoris-agent skill await`,
/// whose order follows the last `await` of the thread.
enum Incoming {
    Event(Event),
    Await,
}

impl<S: Store> Engine<S> {
    pub fn new(store: S) -> Self {
        Engine { store }
    }

    /// The jobs that can run at `now`, in the order they were written.
    pub async fn due_jobs(&mut self, now: u64) -> Result<Vec<JobRecord>, EngineError> {
        Ok(self
            .store
            .jobs()
            .await?
            .into_iter()
            .filter(|j| j.job.due() <= now)
            .collect())
    }

    /// Starts a SkillRun. Its parent is the SkillRun of the thread that entered
    /// `sanchoris-agent skill await` last, if one waits there.
    pub async fn start(
        &mut self,
        request: StartRequest,
        now: u64,
    ) -> Result<Response, EngineError> {
        if let Some(response) = self.store.response(&request.request).await? {
            return Ok(response);
        }
        let waiters: Vec<_> = self
            .store
            .runs_of_thread(&request.thread)
            .await?
            .into_iter()
            .filter_map(|r| lean::waiter_of(r.run))
            .collect();
        let parent = lean::parent_of(waiters);
        let id = self.store.last_run_id().await? + 1;
        let Outcome::Applied { run, write } = lean::start(
            request.definition.clone(),
            nat(id),
            to_lean(&request.input),
            nat(now),
        ) else {
            unreachable!("start always applies")
        };
        let record = RunRecord {
            thread: request.thread,
            origin: request.origin,
            parent,
            definition: request.definition,
            run: run.clone(),
            version: 0,
        };
        let response = run_response(&run);
        let tx = self.transaction(
            record,
            None,
            run,
            write,
            Some((request.request, response.clone())),
            None,
        );
        self.store.commit(tx).await?;
        Ok(response)
    }

    fn transaction(
        &self,
        mut record: RunRecord,
        expected_version: Option<u64>,
        run: Run,
        write: Write,
        request: Option<(String, Response)>,
        await_order: Option<u64>,
    ) -> Transaction {
        let id = u64_of(&run.id);
        let skill = record.definition.skill.clone();
        let mut jobs = Vec::new();
        for x in &write.execs {
            jobs.push(Job::Exec(ExecRequest {
                key: ExecKey {
                    run: id,
                    seq: u64_of(&x.seq),
                    attempt: u64_of(&x.attempt),
                },
                thread: record.thread.clone(),
                skill: skill.clone(),
                command: x.command.clone(),
                stdin: x.stdin.clone(),
                timeout_seconds: u64_of(&x.timeout_seconds),
            }));
        }
        for (timer, due) in &write.timers {
            jobs.push(Job::Timer {
                run: id,
                timer: timer.clone(),
                due: u64_of(due),
            });
        }
        let mut messages: Vec<Message> = write
            .notices
            .iter()
            .map(|n| Message::Notice {
                run: id,
                text: notice::text(&skill, id, n),
            })
            .collect();
        messages.extend(write.outputs.iter().map(|v| Message::ContinueOutput {
            run: id,
            output: from_lean(v),
        }));
        record.version = expected_version.map_or(0, |v| v + 1);
        record.run = run;
        Transaction {
            run: record,
            expected_version,
            entries: write.entries,
            jobs,
            messages,
            request,
            await_order,
        }
    }

    /// Runs one transition of a SkillRun and writes it.
    async fn apply(
        &mut self,
        run: u64,
        incoming: Incoming,
        now: u64,
        request: Option<&str>,
    ) -> Result<(Applied, Option<Response>), EngineError> {
        let record = self
            .store
            .run(run)
            .await?
            .ok_or(EngineError::UnknownRun(run))?;
        let (event, await_order) = match incoming {
            Incoming::Event(event) => (event, None),
            Incoming::Await => {
                let order = self.store.await_order(&record.thread).await? + 1;
                (Event::Await { order: nat(order) }, Some(order))
            }
        };
        let outcome = lean::step(
            record.definition.clone(),
            record.run.clone(),
            event,
            nat(now),
        );
        match outcome {
            Outcome::Applied { run: next, write } => {
                let response = run_response(&next);
                let version = record.version;
                let tx = self.transaction(
                    record,
                    Some(version),
                    next,
                    write,
                    request.map(|r| (r.to_string(), response.clone())),
                    await_order,
                );
                self.store.commit(tx).await?;
                Ok((Applied::Applied, Some(response)))
            }
            Outcome::Ignored => Ok((Applied::Ignored, None)),
            Outcome::Rejected { reason } => Ok((
                Applied::Rejected(reason),
                Some(rejection_response(run, &reason)),
            )),
        }
    }

    /// Takes the result of a command (`exec_result`). The first result of a key is kept and
    /// is the one the transition takes.
    pub async fn exec_result(
        &mut self,
        key: ExecKey,
        output: ExecOutput,
        now: u64,
    ) -> Result<Applied, EngineError> {
        let output = self.store.put_exec_result(key, output).await?;
        self.apply_result(key, output, now).await
    }

    async fn apply_result(
        &mut self,
        key: ExecKey,
        output: ExecOutput,
        now: u64,
    ) -> Result<Applied, EngineError> {
        let event = Event::Exec {
            seq: nat(key.seq),
            attempt: nat(key.attempt),
            output,
        };
        Ok(self
            .apply(key.run, Incoming::Event(event), now, None)
            .await?
            .0)
    }

    /// Takes the start of `sanchoris-agent skill await` in the command of a SkillRun.
    pub async fn await_(
        &mut self,
        request: &str,
        run: u64,
        now: u64,
    ) -> Result<Response, EngineError> {
        if let Some(response) = self.store.response(request).await? {
            return Ok(response);
        }
        let (_, response) = self.apply(run, Incoming::Await, now, Some(request)).await?;
        Ok(response.unwrap_or_else(|| rejection_response(run, &Rejection::NotAwaiting)))
    }

    /// Takes `sanchoris-agent skill continue`.
    pub async fn continue_(
        &mut self,
        request: &str,
        run: u64,
        output: &Value,
        now: u64,
    ) -> Result<Response, EngineError> {
        if let Some(response) = self.store.response(request).await? {
            return Ok(response);
        }
        let event = Event::Continue {
            output: to_lean(output),
        };
        let (_, response) = self
            .apply(run, Incoming::Event(event), now, Some(request))
            .await?;
        Ok(response.unwrap_or_else(|| rejection_response(run, &Rejection::Finished)))
    }

    /// Stops a SkillRun because its thread was stopped.
    pub async fn abort(&mut self, run: u64, now: u64) -> Result<Applied, EngineError> {
        Ok(self
            .apply(run, Incoming::Event(Event::Abort), now, None)
            .await?
            .0)
    }

    /// Runs a job and then deletes it. A command whose key has a result is not sent again:
    /// the transition takes the result.
    pub async fn run_job<W: Workspace>(
        &mut self,
        job: &JobRecord,
        workspace: &mut W,
        now: u64,
    ) -> Result<(), EngineError> {
        match &job.job {
            Job::Exec(request) => match self.store.exec_result(request.key).await? {
                Some(output) => {
                    self.apply_result(request.key, output, now).await?;
                }
                None => workspace.exec(request).await,
            },
            Job::Timer { run, timer, .. } => {
                let event = Event::Timer {
                    timer: timer.clone(),
                };
                self.apply(*run, Incoming::Event(event), now, None).await?;
            }
        }
        self.store.ack(job.id).await?;
        Ok(())
    }

    /// Sends again the commands that the SkillRuns of a thread run, with the same keys, for a
    /// Workspace that was restored. A key with a result is not sent: the transition takes it.
    pub async fn redispatch<W: Workspace>(
        &mut self,
        thread: &str,
        workspace: &mut W,
        now: u64,
    ) -> Result<(), EngineError> {
        for record in self.store.runs_of_thread(thread).await? {
            let Some(e) = record.run.current.clone() else {
                continue;
            };
            if record.run.status != RunStatus::Running
                || !matches!(e.phase, Phase::Executing { .. })
            {
                continue;
            }
            let key = ExecKey {
                run: record.id(),
                seq: u64_of(&e.seq),
                attempt: u64_of(&e.attempts),
            };
            if let Some(output) = self.store.exec_result(key).await? {
                self.apply_result(key, output, now).await?;
                continue;
            }
            if let Some(request) = self.store.exec_request(key).await? {
                workspace.exec(&request).await;
            }
        }
        Ok(())
    }
}
