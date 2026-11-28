//! The domain of sanchoris over DynamoDB and S3: the entities of a User, the unit of work of
//! a User, the state of threads, Trigger evaluation, and the store of the engine.

pub mod activation;
pub mod blob;
pub mod db;
pub mod engine_store;
pub mod entities;
pub mod generated;
pub mod launch;
pub mod local;
pub mod outbox;
pub mod pattern;
pub mod plans;
pub mod schema;
pub mod secret;
pub mod thread;
pub mod token;
pub mod trigger;

pub use generated::sanchoris_host::can_start_workspace;
