//! The integration tests of `sanchoris-core` against DynamoDB Local (`architecture.md`「テスト」):
//! one server for the process, one set of tables, and a new User for each test.

mod common;
mod db;
mod local_limits;
mod store_limits;
mod threads;
mod triggers;
