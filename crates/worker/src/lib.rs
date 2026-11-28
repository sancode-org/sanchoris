//! The control plane's background work and the Thread channel: the dispatcher over the stream
//! of the `main` table, the jobs (thread-starter, workspace-supervisor, engine-worker,
//! thread-supervisor), and the messages of `sanchoris-agent`, against the outside behind
//! [`ports`]. [`local`] runs it all in one process.

pub mod channel;
pub mod deliver;
pub mod jobs;
pub mod local;
pub mod ports;
pub mod workspace;
