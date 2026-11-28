//! What DynamoDB Local enforces of the limits of `TransactWriteItems`, sent without the checks
//! of `UserTx`.

use aws_sdk_dynamodb::types::{AttributeValue, Put, TransactWriteItem};
use sanchoris_core::local;

fn put(table: &str, sk: usize, bytes: usize) -> TransactWriteItem {
    let put = Put::builder()
        .table_name(table)
        .item("pk", AttributeValue::S("U#limits".into()))
        .item("sk", AttributeValue::S(format!("ITEM#{sk:04}")))
        .item("v", AttributeValue::S("x".repeat(bytes)))
        .build()
        .unwrap();
    TransactWriteItem::builder().put(put).build()
}

async fn send(db: &sanchoris_core::db::Db, items: Vec<TransactWriteItem>) -> Result<(), String> {
    db.client()
        .transact_write_items()
        .set_transact_items(Some(items))
        .send()
        .await
        .map(|_| ())
        .map_err(|e| format!("{:?}", e.into_service_error()))
}

#[tokio::test]
async fn dynamodb_local_rejects_101_actions() {
    let db = local::tables().await;
    let t = &db.tables().main;
    assert!(
        send(&db, (0..100).map(|i| put(t, i, 10)).collect())
            .await
            .is_ok()
    );
    let r = send(&db, (0..101).map(|i| put(t, i, 10)).collect()).await;
    assert!(r.is_err(), "101 actions: {r:?}");
}

#[tokio::test]
async fn dynamodb_local_rejects_an_item_over_400_kib() {
    let db = local::tables().await;
    let t = &db.tables().main;
    assert!(send(&db, vec![put(t, 0, 399 * 1024)]).await.is_ok());
    let r = send(&db, vec![put(t, 1, 401 * 1024)]).await;
    assert!(r.is_err(), "an item of 401 KiB: {r:?}");
}

#[tokio::test]
async fn dynamodb_local_rejects_more_than_4_mb() {
    let db = local::tables().await;
    let t = &db.tables().main;
    let r = send(&db, (0..11).map(|i| put(t, i, 390 * 1024)).collect()).await;
    println!("11 items of 390 KiB: {r:?}");
    assert!(r.is_err(), "4.2 MB in one transaction: {r:?}");
}

#[tokio::test]
async fn the_4_mb_of_dynamodb_local_is_4_mib() {
    let db = local::tables().await;
    let t = &db.tables().main;
    // 10 items of 409,000 bytes: 4,090,000 bytes, over 4,000,000 and under 4,194,304.
    let r = send(&db, (0..10).map(|i| put(t, i, 409_000)).collect()).await;
    println!("4,090,000 bytes: {r:?}");
    assert!(r.is_ok(), "{r:?}");
}
