//! A simulated thread for the engine: a Workspace that answers commands from a script, an
//! agent that answers notifications, a store that can fail at a chosen operation, and a clock.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use sanchoris_config::{Outside, validate};
use sanchoris_engine::lean::{Definition, Entry, ExecOutput};
use sanchoris_engine::{
    Engine, EngineError, ExecKey, ExecRequest, JobRecord, Message, Origin, Response, RunRecord,
    StartRequest, Store, StoreError, Transaction, Workspace, definition, exec_output,
};

pub use crate::backend::TestStore;
use serde_json::{Value, json};

pub const THREAD: &str = "th_01JD8Q2M4K";

const CONFIG: &str = include_str!("../../../config/tests/fixtures/sanchoris.yaml");

fn outside() -> Outside {
    Outside {
        credentials: vec!["my-claude".to_string()],
        hosts: Vec::new(),
        repositories: vec!["acme/shop-web".to_string(), "acme/shop-api".to_string()],
    }
}

/// The definitions of the skills of a configuration with the skills `extra` (name and
/// `skill.yaml`) and the built-in ones, after validation.
/// A skill of a configuration: its definition and the `skill.yaml` it was read from.
#[derive(Debug, Clone)]
pub struct SkillSource {
    pub definition: Definition,
    pub source: Value,
}

pub fn skills(extra: &[(&str, &str)]) -> BTreeMap<String, SkillSource> {
    let mut files = BTreeMap::from([("sanchoris.yaml".to_string(), CONFIG.to_string())]);
    for (name, yaml) in extra {
        files.insert(
            format!("skills/{name}/SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: A skill of the engine tests.\n---\n\nRun it with `sanchoris-agent skill run {name}`.\n"
            ),
        );
        files.insert(format!("skills/{name}/skill.yaml"), yaml.to_string());
    }
    let config = validate(&files, &outside()).expect("the configuration passes");
    config
        .skills
        .iter()
        .filter_map(|(name, skill)| {
            let d = skill.definition.as_ref()?;
            Some((
                name.clone(),
                SkillSource {
                    definition: definition(name, &d.source).expect("a definition"),
                    source: d.source.clone(),
                },
            ))
        })
        .collect()
}

pub fn open_pr_green_ci() -> Definition {
    skills(&[])["open-pr-green-ci"].definition.clone()
}

/// A store that fails at the operations whose numbers (from 1) are in `crash_at`, before doing
/// them, as a process that stops there, and after doing those in `crash_after`, as a process
/// that stops before it hears that the write is done.
#[derive(Debug)]
pub struct FaultStore<S> {
    pub inner: S,
    pub ops: usize,
    pub crash_at: BTreeSet<usize>,
    pub crash_after: BTreeSet<usize>,
    pub commits: usize,
    /// The transactions in the order they were written.
    pub log: Vec<Transaction>,
}

impl<S> FaultStore<S> {
    pub fn new(inner: S) -> Self {
        FaultStore {
            inner,
            ops: 0,
            crash_at: BTreeSet::new(),
            crash_after: BTreeSet::new(),
            commits: 0,
            log: Vec::new(),
        }
    }

    fn op(&mut self) -> Result<(), StoreError> {
        self.ops += 1;
        if self.crash_at.contains(&self.ops) {
            Err(StoreError::Unavailable)
        } else {
            Ok(())
        }
    }

    fn done<T>(&self, value: T) -> Result<T, StoreError> {
        if self.crash_after.contains(&self.ops) {
            Err(StoreError::Unavailable)
        } else {
            Ok(value)
        }
    }
}

impl<S: Store> Store for FaultStore<S> {
    async fn commit(&mut self, tx: Transaction) -> Result<(), StoreError> {
        self.op()?;
        let written = tx.clone();
        self.inner.commit(tx).await?;
        self.commits += 1;
        self.log.push(written);
        self.done(())
    }
    async fn run(&mut self, id: u64) -> Result<Option<RunRecord>, StoreError> {
        self.op()?;
        self.inner.run(id).await
    }
    async fn runs_of_thread(&mut self, thread: &str) -> Result<Vec<RunRecord>, StoreError> {
        self.op()?;
        self.inner.runs_of_thread(thread).await
    }
    async fn entries(&mut self, run: u64) -> Result<Vec<Entry>, StoreError> {
        self.op()?;
        self.inner.entries(run).await
    }
    async fn put_exec_result(
        &mut self,
        key: ExecKey,
        output: ExecOutput,
    ) -> Result<ExecOutput, StoreError> {
        self.op()?;
        let kept = self.inner.put_exec_result(key, output).await?;
        self.done(kept)
    }
    async fn exec_result(&mut self, key: ExecKey) -> Result<Option<ExecOutput>, StoreError> {
        self.op()?;
        self.inner.exec_result(key).await
    }
    async fn exec_request(&mut self, key: ExecKey) -> Result<Option<ExecRequest>, StoreError> {
        self.op()?;
        self.inner.exec_request(key).await
    }
    async fn jobs(&mut self) -> Result<Vec<JobRecord>, StoreError> {
        self.op()?;
        self.inner.jobs().await
    }
    async fn ack(&mut self, job: u64) -> Result<(), StoreError> {
        self.op()?;
        self.inner.ack(job).await?;
        self.done(())
    }
    async fn response(&mut self, request: &str) -> Result<Option<Response>, StoreError> {
        self.op()?;
        self.inner.response(request).await
    }
    async fn await_order(&mut self, thread: &str) -> Result<u64, StoreError> {
        self.op()?;
        self.inner.await_order(thread).await
    }
    async fn last_run_id(&mut self) -> Result<u64, StoreError> {
        self.op()?;
        self.inner.last_run_id().await
    }
    async fn messages(&mut self, thread: &str) -> Result<Vec<Message>, StoreError> {
        self.op()?;
        self.inner.messages(thread).await
    }
}

/// A command as the script sees it: the State, the entry into it and the attempt.
#[derive(Debug, Clone)]
pub struct Call {
    pub request: ExecRequest,
    pub state: String,
    pub visit: u64,
    pub attempt: u64,
    pub input: Value,
}

/// What the Workspace does with a command.
#[derive(Debug, Clone)]
pub enum Behavior {
    /// Ends after `after_ms` with the exit code and output.
    Finish {
        after_ms: u64,
        exit: i64,
        stdout: String,
        stderr: String,
    },
    /// Is killed at its `TimeoutSeconds` and answers `timedOut`.
    TimesOut,
    /// Never answers, as a Workspace that is gone.
    Hang,
    /// `sanchoris-agent skill await`: asks the engine after `after_ms` and ends when the Output
    /// of a `continue` arrives.
    Await { after_ms: u64 },
}

pub fn ok(after_ms: u64, stdout: &str) -> Behavior {
    Behavior::Finish {
        after_ms,
        exit: 0,
        stdout: stdout.to_string(),
        stderr: String::new(),
    }
}

pub fn fails(after_ms: u64, exit: i64, stdout: &str, stderr: &str) -> Behavior {
    Behavior::Finish {
        after_ms,
        exit,
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
    }
}

/// What the agent does after a notification: `continue` a SkillRun after `after_ms`, or start
/// a skill.
#[derive(Debug, Clone)]
pub enum Action {
    Continue {
        after_ms: u64,
        run: u64,
        output: Value,
    },
    Start {
        after_ms: u64,
        skill: String,
        input: Value,
    },
}

#[derive(Debug, Clone)]
enum Delivery {
    ExecResult(ExecKey, ExecOutput),
    Await {
        key: ExecKey,
        request: String,
    },
    Continue {
        run: u64,
        output: Value,
        request: String,
    },
    Start {
        skill: String,
        input: Value,
        request: String,
    },
}

pub type Script = Box<dyn FnMut(&Call) -> Behavior>;
pub type Agent = Box<dyn FnMut(u64, &str) -> Vec<Action>>;

#[derive(Default)]
struct Recorder {
    requests: Vec<ExecRequest>,
}

impl Workspace for Recorder {
    async fn exec(&mut self, request: &ExecRequest) {
        self.requests.push(request.clone());
    }
}

pub struct Sim {
    pub engine: Engine<FaultStore<TestStore>>,
    pub now: u64,
    pub skills: BTreeMap<String, SkillSource>,
    script: Script,
    agent: Agent,
    deliveries: Vec<(u64, u64, Delivery)>,
    next_delivery: u64,
    /// The commands the Workspace runs; a key runs once, as `sanchoris-agent` answers a key
    /// from its first run.
    awaiting: BTreeMap<u64, ExecKey>,
    started: BTreeSet<ExecKey>,
    /// How many times the engine sent each key.
    pub sent: BTreeMap<ExecKey, usize>,
    /// The commands in the order the Workspace started them.
    pub calls: Vec<Call>,
    /// The answers of `await`, `continue` and starts.
    pub responses: Vec<(String, Response)>,
    seen: usize,
    pub crashes: usize,
    requests: u64,
    /// How long an `await` takes to end after the Output arrives.
    pub await_answer_after_ms: u64,
}

impl Sim {
    pub async fn new(skills: BTreeMap<String, SkillSource>, script: Script, agent: Agent) -> Sim {
        Sim {
            engine: Engine::new(FaultStore::new(crate::backend::store().await)),
            now: 1_795_858_200_000,
            skills,
            script,
            agent,
            deliveries: Vec::new(),
            next_delivery: 0,
            awaiting: BTreeMap::new(),
            started: BTreeSet::new(),
            sent: BTreeMap::new(),
            calls: Vec::new(),
            responses: Vec::new(),
            seen: 0,
            crashes: 0,
            requests: 0,
            await_answer_after_ms: 0,
        }
    }

    /// The store under the one that fails, read without counting operations.
    pub fn store(&mut self) -> &mut TestStore {
        &mut self.engine.store.inner
    }

    pub async fn outbox(&mut self) -> Vec<Message> {
        self.store().messages(THREAD).await.expect("the outbox")
    }

    pub async fn has_exec_result(&mut self, key: ExecKey) -> bool {
        self.store()
            .exec_result(key)
            .await
            .expect("a result")
            .is_some()
    }

    fn deliver_at(&mut self, at: u64, d: Delivery) {
        self.next_delivery += 1;
        self.deliveries.push((at, self.next_delivery, d));
    }

    fn request_id(&mut self) -> String {
        self.requests += 1;
        format!("req-{}", self.requests)
    }

    /// Starts a skill from the agent of the thread now.
    pub fn start(&mut self, skill: &str, input: Value) {
        let request = self.request_id();
        let now = self.now;
        self.deliver_at(
            now,
            Delivery::Start {
                skill: skill.to_string(),
                input,
                request,
            },
        );
    }

    /// `continue` from the agent of the thread after `after_ms`.
    pub fn continue_(&mut self, after_ms: u64, run: u64, output: Value) {
        let request = self.request_id();
        let at = self.now + after_ms;
        self.deliver_at(
            at,
            Delivery::Continue {
                run,
                output,
                request,
            },
        );
    }

    fn crashed<T>(&mut self, r: Result<T, EngineError>) -> Option<T> {
        match r {
            Ok(v) => Some(v),
            Err(EngineError::Store(StoreError::Unavailable)) => {
                self.crashes += 1;
                None
            }
            Err(e) => panic!("the engine failed: {e}"),
        }
    }

    /// Runs one delivery; false when the engine stopped and it has to be sent again.
    async fn run_delivery(&mut self, d: &Delivery) -> bool {
        let now = self.now;
        match d {
            Delivery::ExecResult(key, output) => {
                let r = self.engine.exec_result(*key, output.clone(), now).await;
                self.crashed(r).is_some()
            }
            Delivery::Await { key, request } => {
                let r = self.engine.await_(request, key.run, now).await;
                match self.crashed(r) {
                    Some(response) => {
                        if let Response::Error { message, .. } = &response {
                            let out = exec_output(1, "", message, false);
                            self.deliver_at(now, Delivery::ExecResult(*key, out));
                        }
                        self.responses.push((request.clone(), response));
                        true
                    }
                    None => false,
                }
            }
            Delivery::Continue {
                run,
                output,
                request,
            } => {
                let r = self.engine.continue_(request, *run, output, now).await;
                match self.crashed(r) {
                    Some(response) => {
                        self.responses.push((request.clone(), response));
                        true
                    }
                    None => false,
                }
            }
            Delivery::Start {
                skill,
                input,
                request,
            } => {
                let start = StartRequest {
                    request: request.clone(),
                    thread: THREAD.to_string(),
                    origin: Origin::Thread,
                    definition: self.skills[skill].definition.clone(),
                    source: self.skills[skill].source.clone(),
                    input: input.clone(),
                };
                let r = self.engine.start(start, now).await;
                match self.crashed(r) {
                    Some(response) => {
                        self.responses.push((request.clone(), response));
                        true
                    }
                    None => false,
                }
            }
        }
    }

    async fn start_command(&mut self, request: ExecRequest) {
        *self.sent.entry(request.key).or_default() += 1;
        if !self.started.insert(request.key) {
            return;
        }
        let entry = self
            .entries(request.key.run)
            .await
            .into_iter()
            .find(|e| sanchoris_engine::json::u64_of(&e.seq) == request.key.seq)
            .expect("the entry of a command");
        let call = Call {
            state: entry.state.clone(),
            visit: sanchoris_engine::json::u64_of(&entry.visit),
            attempt: request.key.attempt,
            input: serde_json::from_str(&request.stdin).expect("the stdin is JSON"),
            request: request.clone(),
        };
        self.calls.push(call.clone());
        let now = self.now;
        match (self.script)(&call) {
            Behavior::Finish {
                after_ms,
                exit,
                stdout,
                stderr,
            } => self.deliver_at(
                now + after_ms,
                Delivery::ExecResult(request.key, exec_output(exit, &stdout, &stderr, false)),
            ),
            Behavior::TimesOut => self.deliver_at(
                now + request.timeout_seconds * 1000,
                Delivery::ExecResult(request.key, exec_output(-1, "", "", true)),
            ),
            Behavior::Hang => {}
            Behavior::Await { after_ms } => {
                self.awaiting.insert(request.key.run, request.key);
                let id = self.request_id();
                self.deliver_at(
                    now + after_ms,
                    Delivery::Await {
                        key: request.key,
                        request: id,
                    },
                );
            }
        }
    }

    /// Reads the new messages of the thread: the agent answers notifications, and an `await`
    /// ends with the Output that arrives for it.
    async fn read_messages(&mut self) -> bool {
        let messages = self.outbox().await;
        let fresh: Vec<Message> = messages[self.seen..].to_vec();
        self.seen = messages.len();
        let now = self.now;
        for m in &fresh {
            match m {
                Message::Notice { run, text } => {
                    for action in (self.agent)(*run, text) {
                        match action {
                            Action::Continue {
                                after_ms,
                                run,
                                output,
                            } => self.continue_(after_ms, run, output),
                            Action::Start {
                                after_ms,
                                skill,
                                input,
                            } => {
                                let request = self.request_id();
                                self.deliver_at(
                                    now + after_ms,
                                    Delivery::Start {
                                        skill,
                                        input,
                                        request,
                                    },
                                );
                            }
                        }
                    }
                }
                Message::ContinueOutput { run, output } => {
                    if let Some(key) = self.awaiting.remove(run) {
                        let out = exec_output(0, &format!("{output}\n"), "", false);
                        let at = now + self.await_answer_after_ms;
                        self.deliver_at(at, Delivery::ExecResult(key, out));
                    }
                }
            }
        }
        !fresh.is_empty()
    }

    /// Runs until nothing is left to do, or `until` (milliseconds of the clock) is reached.
    pub async fn run_until(&mut self, until: u64) {
        let mut rounds = 0;
        loop {
            rounds += 1;
            assert!(rounds < 100_000, "the simulation does not end");
            let now = self.now;
            if let Some(i) = self
                .deliveries
                .iter()
                .enumerate()
                .filter(|(_, (at, _, _))| *at <= now)
                .min_by_key(|(_, (at, seq, _))| (*at, *seq))
                .map(|(i, _)| i)
            {
                let (_, _, d) = self.deliveries[i].clone();
                if self.run_delivery(&d).await {
                    self.deliveries.remove(i);
                }
                self.read_messages().await;
                continue;
            }
            let jobs = self.engine.due_jobs(now).await;
            let Some(jobs) = self.crashed(jobs) else {
                continue;
            };
            if let Some(job) = jobs.first() {
                let mut recorder = Recorder::default();
                let r = self.engine.run_job(job, &mut recorder, now).await;
                for request in recorder.requests {
                    self.start_command(request).await;
                }
                self.crashed(r);
                self.read_messages().await;
                continue;
            }
            if self.read_messages().await {
                continue;
            }
            let next_delivery = self.deliveries.iter().map(|(at, _, _)| *at).min();
            let next_job = self
                .store()
                .jobs()
                .await
                .expect("the jobs")
                .iter()
                .map(|j| j.job.due())
                .min();
            let next = match (next_delivery, next_job) {
                (Some(a), Some(b)) => a.min(b),
                (Some(a), None) | (None, Some(a)) => a,
                (None, None) => return,
            };
            if next > until {
                return;
            }
            self.now = next;
        }
    }

    pub async fn run(&mut self) {
        self.run_until(u64::MAX).await;
    }

    /// Sends again the commands that run, as for a Workspace that was restored.
    pub async fn restore_workspace(&mut self) {
        let mut recorder = Recorder::default();
        let now = self.now;
        self.engine
            .redispatch(THREAD, &mut recorder, now)
            .await
            .expect("redispatch");
        for request in recorder.requests {
            *self.sent.entry(request.key).or_default() += 1;
        }
    }

    pub async fn notices(&mut self) -> Vec<String> {
        self.outbox()
            .await
            .into_iter()
            .filter_map(|m| match m {
                Message::Notice { text, .. } => Some(text),
                _ => None,
            })
            .collect()
    }

    pub async fn run_record(&mut self, id: u64) -> RunRecord {
        self.store()
            .run(id)
            .await
            .expect("the SkillRun")
            .expect("a SkillRun")
    }

    pub async fn entries(&mut self, run: u64) -> Vec<Entry> {
        self.store().entries(run).await.expect("the entries")
    }

    /// The StateEntries of every SkillRun of the store, by SkillRun.
    pub async fn all_entries(&mut self) -> Vec<Vec<Entry>> {
        let last = self.store().last_run_id().await.expect("the last SkillRun");
        let mut all = Vec::new();
        for run in 1..=last {
            all.push(self.entries(run).await);
        }
        all
    }

    /// The States of the StateEntries of a SkillRun with their statuses.
    pub async fn path(&mut self, run: u64) -> Vec<(String, String)> {
        self.entries(run)
            .await
            .iter()
            .map(|e| (e.state.clone(), format!("{:?}", e.status)))
            .collect()
    }
}

pub fn json_out(v: &Value) -> String {
    format!("{v}\n")
}

pub fn pr() -> Value {
    json!({"number": 52, "url": "https://github.com/acme/shop-web/pull/52"})
}
