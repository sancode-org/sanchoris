//! What the engine keeps, and the store it keeps it in.
//!
//! A transition is one [`Transaction`]: the SkillRun, the StateEntries it writes, the jobs it
//! schedules, the messages of the thread and the answer to the request that caused it. The
//! result of a command is written on its own, by its idempotency key, before the transition that
//! takes it.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde_json::Value;

use crate::generated::sanchoris_engine::{Definition, Entry, ExecOutput, Parent, Run, Timer};

/// The idempotency key of a command: the SkillRun, the StateEntry and the attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExecKey {
    pub run: u64,
    pub seq: u64,
    pub attempt: u64,
}

impl fmt::Display for ExecKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.run, self.seq, self.attempt)
    }
}

impl FromStr for ExecKey {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        let [run, seq, attempt] = parts.as_slice() else {
            return Err(format!("{s} is not an idempotency key"));
        };
        let parse = |p: &str| {
            p.parse::<u64>()
                .map_err(|_| format!("{s} is not an idempotency key"))
        };
        Ok(ExecKey {
            run: parse(run)?,
            seq: parse(seq)?,
            attempt: parse(attempt)?,
        })
    }
}

/// Who started a SkillRun.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The agent of the thread, with `sanchoris-agent skill run`.
    Thread,
    /// The User, with `/<name>`.
    User,
}

/// A SkillRun with the thread it runs in, its parent and the definition it runs.
#[derive(Debug, Clone, PartialEq)]
pub struct RunRecord {
    pub thread: String,
    pub origin: Origin,
    pub parent: Option<Parent>,
    pub definition: Definition,
    pub run: Run,
    /// Goes up by one with each transaction; a transaction is written only over the version it
    /// read.
    pub version: u64,
}

impl RunRecord {
    pub fn id(&self) -> u64 {
        crate::json::u64_of(&self.run.id)
    }
}

/// A command for the Workspace of a thread (`exec`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecRequest {
    pub key: ExecKey,
    pub thread: String,
    pub skill: String,
    pub command: String,
    pub stdin: String,
    pub timeout_seconds: u64,
}

/// Work for later: send a command, or take a timer when it is due.
#[derive(Debug, Clone, PartialEq)]
pub enum Job {
    Exec(ExecRequest),
    Timer { run: u64, timer: Timer, due: u64 },
}

impl Job {
    /// When the job can run.
    pub fn due(&self) -> u64 {
        match self {
            Job::Exec(_) => 0,
            Job::Timer { due, .. } => *due,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobRecord {
    pub id: u64,
    pub job: Job,
}

/// A message to the thread, in the order of its outbox.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// A notification of a SkillRun, as an input of the thread.
    Notice { run: u64, text: String },
    /// The Output of a `continue` for `sanchoris-agent skill await` (`continue_output`).
    ContinueOutput { run: u64, output: Value },
}

/// The answer to a `skill_request` (`skill_response`).
#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    Ok(Value),
    Error { code: String, message: String },
}

/// One write of a transition.
#[derive(Debug, Clone, PartialEq)]
pub struct Transaction {
    pub run: RunRecord,
    /// The version the transition read; `None` for a new SkillRun.
    pub expected_version: Option<u64>,
    pub entries: Vec<Entry>,
    pub jobs: Vec<Job>,
    pub messages: Vec<Message>,
    /// The request the transition answers, and its answer.
    pub request: Option<(String, Response)>,
    /// The new order of the last `await` of the thread.
    pub await_order: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("SkillRun {0} changed since it was read")]
    Conflict(u64),
    #[error("the store is unavailable")]
    Unavailable,
}

/// The store of SkillRuns, StateEntries, command results, jobs and the outboxes of threads.
pub trait Store {
    fn commit(&mut self, tx: Transaction) -> Result<(), StoreError>;
    fn run(&mut self, id: u64) -> Result<Option<RunRecord>, StoreError>;
    fn runs_of_thread(&mut self, thread: &str) -> Result<Vec<RunRecord>, StoreError>;
    fn entries(&mut self, run: u64) -> Result<Vec<Entry>, StoreError>;
    /// Writes the result of a command unless the key has one, and returns the one it keeps.
    fn put_exec_result(
        &mut self,
        key: ExecKey,
        output: ExecOutput,
    ) -> Result<ExecOutput, StoreError>;
    fn exec_result(&mut self, key: ExecKey) -> Result<Option<ExecOutput>, StoreError>;
    /// The command that a transaction sent with the key.
    fn exec_request(&mut self, key: ExecKey) -> Result<Option<ExecRequest>, StoreError>;
    fn jobs(&mut self) -> Result<Vec<JobRecord>, StoreError>;
    fn ack(&mut self, job: u64) -> Result<(), StoreError>;
    fn response(&mut self, request: &str) -> Result<Option<Response>, StoreError>;
    /// The order of the last `await` of the thread, 0 before the first.
    fn await_order(&mut self, thread: &str) -> Result<u64, StoreError>;
    /// The ID of the last SkillRun; a new one is the next number, and its transaction fails
    /// with a conflict when another took it.
    fn last_run_id(&mut self) -> Result<u64, StoreError>;
    fn messages(&mut self, thread: &str) -> Result<Vec<Message>, StoreError>;
}

/// A store in memory.
#[derive(Debug, Clone, Default)]
pub struct MemoryStore {
    pub runs: BTreeMap<u64, RunRecord>,
    pub entries: BTreeMap<(u64, u64), Entry>,
    pub exec_results: BTreeMap<ExecKey, ExecOutput>,
    pub exec_requests: BTreeMap<ExecKey, ExecRequest>,
    pub jobs: BTreeMap<u64, Job>,
    pub responses: BTreeMap<String, Response>,
    pub await_orders: BTreeMap<String, u64>,
    pub outboxes: BTreeMap<String, Vec<Message>>,
    pub last_job: u64,
    pub last_run: u64,
    /// The transactions in the order they were written.
    pub log: Vec<Transaction>,
}

impl Store for MemoryStore {
    fn commit(&mut self, tx: Transaction) -> Result<(), StoreError> {
        let id = tx.run.id();
        let current = self.runs.get(&id).map(|r| r.version);
        if current != tx.expected_version || (current.is_none() && id != self.last_run + 1) {
            return Err(StoreError::Conflict(id));
        }
        self.last_run = self.last_run.max(id);
        let thread = tx.run.thread.clone();
        self.runs.insert(id, tx.run.clone());
        for e in &tx.entries {
            self.entries
                .insert((id, crate::json::u64_of(&e.seq)), e.clone());
        }
        for job in &tx.jobs {
            if let Job::Exec(request) = job {
                self.exec_requests.insert(request.key, request.clone());
            }
            self.last_job += 1;
            self.jobs.insert(self.last_job, job.clone());
        }
        self.outboxes
            .entry(thread.clone())
            .or_default()
            .extend(tx.messages.iter().cloned());
        if let Some((request, response)) = &tx.request {
            self.responses.insert(request.clone(), response.clone());
        }
        if let Some(order) = tx.await_order {
            self.await_orders.insert(thread, order);
        }
        self.log.push(tx);
        Ok(())
    }

    fn run(&mut self, id: u64) -> Result<Option<RunRecord>, StoreError> {
        Ok(self.runs.get(&id).cloned())
    }

    fn runs_of_thread(&mut self, thread: &str) -> Result<Vec<RunRecord>, StoreError> {
        Ok(self
            .runs
            .values()
            .filter(|r| r.thread == thread)
            .cloned()
            .collect())
    }

    fn entries(&mut self, run: u64) -> Result<Vec<Entry>, StoreError> {
        Ok(self
            .entries
            .range((run, 0)..=(run, u64::MAX))
            .map(|(_, e)| e.clone())
            .collect())
    }

    fn put_exec_result(
        &mut self,
        key: ExecKey,
        output: ExecOutput,
    ) -> Result<ExecOutput, StoreError> {
        Ok(self.exec_results.entry(key).or_insert(output).clone())
    }

    fn exec_result(&mut self, key: ExecKey) -> Result<Option<ExecOutput>, StoreError> {
        Ok(self.exec_results.get(&key).cloned())
    }

    fn exec_request(&mut self, key: ExecKey) -> Result<Option<ExecRequest>, StoreError> {
        Ok(self.exec_requests.get(&key).cloned())
    }

    fn jobs(&mut self) -> Result<Vec<JobRecord>, StoreError> {
        Ok(self
            .jobs
            .iter()
            .map(|(id, job)| JobRecord {
                id: *id,
                job: job.clone(),
            })
            .collect())
    }

    fn ack(&mut self, job: u64) -> Result<(), StoreError> {
        self.jobs.remove(&job);
        Ok(())
    }

    fn response(&mut self, request: &str) -> Result<Option<Response>, StoreError> {
        Ok(self.responses.get(request).cloned())
    }

    fn await_order(&mut self, thread: &str) -> Result<u64, StoreError> {
        Ok(self.await_orders.get(thread).copied().unwrap_or(0))
    }

    fn last_run_id(&mut self) -> Result<u64, StoreError> {
        Ok(self.last_run)
    }

    fn messages(&mut self, thread: &str) -> Result<Vec<Message>, StoreError> {
        Ok(self.outboxes.get(thread).cloned().unwrap_or_default())
    }
}
