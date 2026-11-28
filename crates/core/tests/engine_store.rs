//! The engine suites (`crates/engine/tests/suite`) against the store on DynamoDB Local.

mod backend {
    use std::sync::atomic::{AtomicU64, Ordering};

    use sanchoris_core::blob::Blobs;
    use sanchoris_core::db::Db;
    use sanchoris_core::engine_store::DynamoStore;

    pub type TestStore = DynamoStore;

    static USERS: AtomicU64 = AtomicU64::new(0);

    /// A store of a new User, on the tables of the test process.
    pub async fn store() -> TestStore {
        let db: Db = sanchoris_core::local::shared_tables().await;
        let user = format!("engine{}", USERS.fetch_add(1, Ordering::SeqCst));
        DynamoStore::new(db, Blobs::memory(), &user)
    }
}

#[path = "../../engine/tests/common/mod.rs"]
mod common;
#[path = "../../engine/tests/suite/mod.rs"]
mod suite;
