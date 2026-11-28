//! The interpreter of `skill.yaml` (`skill.md`「実行エンジン」): the transitions are the Lean
//! module `Sanchoris.Engine`, generated into [`generated`], and this crate runs them against a
//! [`store::Store`] and a [`engine::Workspace`].

pub mod definition;
pub mod engine;
pub mod generated;
pub mod json;
pub mod notice;
pub mod store;

pub use definition::{DefinitionError, definition};
pub use engine::{Applied, Engine, EngineError, StartRequest, Workspace, exec_output};
pub use generated::sanchoris_engine as lean;
pub use store::{
    ExecKey, ExecRequest, Job, JobRecord, MemoryStore, Message, Origin, Response, RunRecord, Store,
    StoreError, Transaction,
};
