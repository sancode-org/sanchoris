//! The engine suites against the store in memory.

mod backend {
    pub type TestStore = sanchoris_engine::MemoryStore;

    pub async fn store() -> TestStore {
        TestStore::default()
    }
}

mod common;
mod suite;
