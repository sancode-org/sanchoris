//! DynamoDB: the `main` and `audit` tables, the items of a User under `U#<user>`, and the
//! unit of work of a User ([`UserTx`]) that writes one `TransactWriteItems` and rejects the keys
//! of other Users.

use std::collections::{BTreeSet, HashMap};

use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_dynamodb::types::{
    AttributeValue, ConditionCheck, Delete, KeysAndAttributes, Put, TransactWriteItem, Update,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Number, Value};

pub type Item = HashMap<String, AttributeValue>;

/// The most actions of one `TransactWriteItems`.
pub const MAX_ACTIONS: usize = 100;
/// The largest item.
pub const MAX_ITEM_BYTES: usize = 400 * 1024;
/// The largest `TransactWriteItems`, all items together: DynamoDB writes 4 MB, which DynamoDB
/// Local counts as 4 MiB; sanchoris keeps to 4,000,000 bytes.
pub const MAX_TRANSACTION_BYTES: usize = 4_000_000;

/// The attributes of the keys and indexes, which an entity does not hold.
pub const KEY_ATTRIBUTES: [&str; 8] = [
    "pk",
    "sk",
    "kind",
    "gsi1pk",
    "gsi1sk",
    "gsi2pk",
    "gsi2sk",
    "expires_at",
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DbError {
    /// A condition of the write failed; `index` is the action whose condition failed.
    #[error("the condition of action {index} failed")]
    Condition { index: usize },
    /// Another transaction wrote the same items at the same time; the write can be retried.
    #[error("another transaction wrote the same items")]
    Conflict,
    #[error("the key {pk} {sk} does not belong to User {user}")]
    ForeignKey {
        user: String,
        pk: String,
        sk: String,
    },
    #[error("the transaction writes {0} twice")]
    SameItem(String),
    #[error("the transaction is too large: {0}")]
    TooLarge(String),
    #[error("an item is not valid: {0}")]
    Invalid(String),
    #[error("a required environment variable is missing: {0}")]
    MissingEnv(&'static str),
    #[error("DynamoDB is unavailable: {0}")]
    Unavailable(String),
}

impl DbError {
    pub fn is_condition(&self) -> bool {
        matches!(self, DbError::Condition { .. })
    }
}

/// The names of the tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tables {
    pub main: String,
    pub audit: String,
}

impl Tables {
    /// The names from `SANCHORIS_TABLE_MAIN` and `SANCHORIS_TABLE_AUDIT` (`infra/tables.json`).
    pub fn from_env() -> Result<Self, DbError> {
        let read = |name: &'static str| std::env::var(name).map_err(|_| DbError::MissingEnv(name));
        Ok(Tables {
            main: read("SANCHORIS_TABLE_MAIN")?,
            audit: read("SANCHORIS_TABLE_AUDIT")?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Table {
    Main,
    Audit,
}

/// The key of an item.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key {
    pub table: TableKey,
    pub pk: String,
    pub sk: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TableKey {
    Main,
    Audit,
}

impl Key {
    pub fn main(pk: impl Into<String>, sk: impl Into<String>) -> Key {
        Key {
            table: TableKey::Main,
            pk: pk.into(),
            sk: sk.into(),
        }
    }

    pub fn audit(pk: impl Into<String>, sk: impl Into<String>) -> Key {
        Key {
            table: TableKey::Audit,
            pk: pk.into(),
            sk: sk.into(),
        }
    }

    fn attributes(&self) -> Item {
        HashMap::from([
            ("pk".to_string(), AttributeValue::S(self.pk.clone())),
            ("sk".to_string(), AttributeValue::S(self.sk.clone())),
        ])
    }
}

/// A JSON value as a DynamoDB value.
pub fn to_attribute(value: &Value) -> AttributeValue {
    match value {
        Value::Null => AttributeValue::Null(true),
        Value::Bool(b) => AttributeValue::Bool(*b),
        Value::Number(n) => AttributeValue::N(n.to_string()),
        Value::String(s) => AttributeValue::S(s.clone()),
        Value::Array(items) => AttributeValue::L(items.iter().map(to_attribute).collect()),
        Value::Object(fields) => AttributeValue::M(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), to_attribute(v)))
                .collect(),
        ),
    }
}

/// A DynamoDB value as a JSON value. The fields of a map are sorted by name.
pub fn from_attribute(value: &AttributeValue) -> Result<Value, DbError> {
    Ok(match value {
        AttributeValue::Null(_) => Value::Null,
        AttributeValue::Bool(b) => Value::Bool(*b),
        AttributeValue::N(n) => {
            let number: Number = n
                .parse()
                .map_err(|_| DbError::Invalid(format!("{n} is not a number")))?;
            Value::Number(number)
        }
        AttributeValue::S(s) => Value::String(s.clone()),
        AttributeValue::L(items) => {
            Value::Array(items.iter().map(from_attribute).collect::<Result<_, _>>()?)
        }
        AttributeValue::M(fields) => {
            let mut names: Vec<&String> = fields.keys().collect();
            names.sort();
            let mut map = Map::new();
            for name in names {
                map.insert(name.clone(), from_attribute(&fields[name])?);
            }
            Value::Object(map)
        }
        other => return Err(DbError::Invalid(format!("unsupported value {other:?}"))),
    })
}

/// The bytes DynamoDB counts for a value.
pub fn attribute_size(value: &AttributeValue) -> usize {
    match value {
        AttributeValue::S(s) => s.len(),
        AttributeValue::N(n) => n.len().div_ceil(2) + 1,
        AttributeValue::B(b) => b.as_ref().len(),
        AttributeValue::Bool(_) | AttributeValue::Null(_) => 1,
        AttributeValue::L(items) => 3 + items.iter().map(|v| 1 + attribute_size(v)).sum::<usize>(),
        AttributeValue::M(fields) => {
            3 + fields
                .iter()
                .map(|(k, v)| 1 + k.len() + attribute_size(v))
                .sum::<usize>()
        }
        AttributeValue::Ss(items) => items.iter().map(String::len).sum(),
        AttributeValue::Ns(items) => items.iter().map(|n| n.len().div_ceil(2) + 1).sum(),
        AttributeValue::Bs(items) => items.iter().map(|b| b.as_ref().len()).sum(),
        _ => 0,
    }
}

/// The bytes DynamoDB counts for an item: the names and the values of its attributes.
pub fn item_size(item: &Item) -> usize {
    item.iter()
        .map(|(name, value)| name.len() + attribute_size(value))
        .sum()
}

/// An item with its key, its kind, and the fields of a value.
pub fn item_of<T: Serialize>(key: &Key, kind: &str, value: &T) -> Result<Item, DbError> {
    let json = serde_json::to_value(value).map_err(|e| DbError::Invalid(e.to_string()))?;
    let Value::Object(fields) = json else {
        return Err(DbError::Invalid(format!("{kind} is not a record")));
    };
    let mut item = key.attributes();
    item.insert("kind".to_string(), AttributeValue::S(kind.to_string()));
    for (name, value) in fields {
        if KEY_ATTRIBUTES.contains(&name.as_str()) {
            return Err(DbError::Invalid(format!(
                "{kind} has the key attribute {name}"
            )));
        }
        item.insert(name, to_attribute(&value));
    }
    Ok(item)
}

/// The value of the fields of an item, without its key and indexes.
pub fn value_of<T: DeserializeOwned>(item: &Item) -> Result<T, DbError> {
    let mut map = Map::new();
    let mut names: Vec<&String> = item.keys().collect();
    names.sort();
    for name in names {
        if !KEY_ATTRIBUTES.contains(&name.as_str()) {
            map.insert(name.clone(), from_attribute(&item[name])?);
        }
    }
    serde_json::from_value(Value::Object(map)).map_err(|e| DbError::Invalid(e.to_string()))
}

pub fn s(value: impl Into<String>) -> AttributeValue {
    AttributeValue::S(value.into())
}

pub fn n(value: impl ToString) -> AttributeValue {
    AttributeValue::N(value.to_string())
}

pub fn get_s<'a>(item: &'a Item, name: &str) -> Option<&'a str> {
    item.get(name)
        .and_then(|v| v.as_s().ok())
        .map(String::as_str)
}

pub fn get_n(item: &Item, name: &str) -> Option<i64> {
    item.get(name)
        .and_then(|v| v.as_n().ok())
        .and_then(|n| n.parse().ok())
}

/// A condition expression with its names and values.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cond {
    expression: String,
    names: HashMap<String, String>,
    values: HashMap<String, AttributeValue>,
}

impl Cond {
    fn with(expression: String, names: &[&str], values: Vec<AttributeValue>) -> Cond {
        let mut c = Cond::default();
        let mut e = expression;
        for (i, name) in names.iter().enumerate() {
            let placeholder = format!("#n{i}");
            e = e.replace(&format!("{{n{i}}}"), &placeholder);
            c.names.insert(placeholder, name.to_string());
        }
        for (i, value) in values.into_iter().enumerate() {
            let placeholder = format!(":v{i}");
            e = e.replace(&format!("{{v{i}}}"), &placeholder);
            c.values.insert(placeholder, value);
        }
        c.expression = e;
        c
    }

    /// The item does not exist.
    pub fn not_exists() -> Cond {
        Cond::with("attribute_not_exists({n0})".into(), &["pk"], vec![])
    }

    /// The item exists.
    pub fn exists() -> Cond {
        Cond::with("attribute_exists({n0})".into(), &["pk"], vec![])
    }

    pub fn missing(name: &str) -> Cond {
        Cond::with("attribute_not_exists({n0})".into(), &[name], vec![])
    }

    pub fn eq(name: &str, value: AttributeValue) -> Cond {
        Cond::with("{n0} = {v0}".into(), &[name], vec![value])
    }

    pub fn ne(name: &str, value: AttributeValue) -> Cond {
        Cond::with("{n0} <> {v0}".into(), &[name], vec![value])
    }

    pub fn lt(name: &str, value: i64) -> Cond {
        Cond::with("{n0} < {v0}".into(), &[name], vec![n(value)])
    }

    pub fn ge(name: &str, value: i64) -> Cond {
        Cond::with("{n0} >= {v0}".into(), &[name], vec![n(value)])
    }

    /// The attribute is missing or below `value`.
    pub fn missing_or_lt(name: &str, value: i64) -> Cond {
        Cond::missing(name).or(Cond::lt(name, value))
    }

    fn join(self, other: Cond, op: &str) -> Cond {
        let mut out = Cond::default();
        let rename = |c: Cond, prefix: &str, out: &mut Cond| -> String {
            let mut e = c.expression;
            let mut names: Vec<_> = c.names.into_iter().collect();
            names.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(b.0.cmp(&a.0)));
            for (placeholder, name) in names {
                let renamed = format!("#{prefix}{}", &placeholder[1..]);
                e = e.replace(&placeholder, &renamed);
                out.names.insert(renamed, name);
            }
            let mut values: Vec<_> = c.values.into_iter().collect();
            values.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(b.0.cmp(&a.0)));
            for (placeholder, value) in values {
                let renamed = format!(":{prefix}{}", &placeholder[1..]);
                e = e.replace(&placeholder, &renamed);
                out.values.insert(renamed, value);
            }
            e
        };
        let left = rename(self, "a", &mut out);
        let right = rename(other, "b", &mut out);
        out.expression = format!("({left}) {op} ({right})");
        out
    }

    pub fn and(self, other: Cond) -> Cond {
        self.join(other, "AND")
    }

    pub fn or(self, other: Cond) -> Cond {
        self.join(other, "OR")
    }
}

/// An update expression: `SET`, `ADD` and `REMOVE`, with a condition.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Change {
    set: Vec<(String, AttributeValue)>,
    add: Vec<(String, i64)>,
    remove: Vec<String>,
    condition: Option<Cond>,
}

impl Change {
    pub fn new() -> Change {
        Change::default()
    }

    pub fn set(mut self, name: &str, value: AttributeValue) -> Change {
        self.set.push((name.to_string(), value));
        self
    }

    pub fn add(mut self, name: &str, delta: i64) -> Change {
        self.add.push((name.to_string(), delta));
        self
    }

    pub fn remove(mut self, name: &str) -> Change {
        self.remove.push(name.to_string());
        self
    }

    pub fn when(mut self, condition: Cond) -> Change {
        self.condition = Some(match self.condition.take() {
            Some(c) => c.and(condition),
            None => condition,
        });
        self
    }

    fn expression(
        &self,
    ) -> (
        String,
        HashMap<String, String>,
        HashMap<String, AttributeValue>,
    ) {
        let mut names = HashMap::new();
        let mut values = HashMap::new();
        let mut parts = Vec::new();
        let name_of = |name: &str, names: &mut HashMap<String, String>| {
            let placeholder = format!("#u{}", names.len());
            names.insert(placeholder.clone(), name.to_string());
            placeholder
        };
        if !self.set.is_empty() {
            let mut sets = Vec::new();
            for (name, value) in &self.set {
                let p = name_of(name, &mut names);
                let v = format!(":u{}", values.len());
                values.insert(v.clone(), value.clone());
                sets.push(format!("{p} = {v}"));
            }
            parts.push(format!("SET {}", sets.join(", ")));
        }
        if !self.add.is_empty() {
            let mut adds = Vec::new();
            for (name, delta) in &self.add {
                let p = name_of(name, &mut names);
                let v = format!(":u{}", values.len());
                values.insert(v.clone(), n(*delta));
                adds.push(format!("{p} {v}"));
            }
            parts.push(format!("ADD {}", adds.join(", ")));
        }
        if !self.remove.is_empty() {
            let removes: Vec<String> = self
                .remove
                .iter()
                .map(|name| name_of(name, &mut names))
                .collect();
            parts.push(format!("REMOVE {}", removes.join(", ")));
        }
        (parts.join(" "), names, values)
    }
}

/// The DynamoDB tables of sanchoris.
#[derive(Debug, Clone)]
pub struct Db {
    client: Client,
    tables: Tables,
}

impl Db {
    pub fn new(client: Client, tables: Tables) -> Db {
        Db { client, tables }
    }

    /// The tables of the environment, with the AWS configuration of the environment.
    pub async fn from_env() -> Result<Db, DbError> {
        let tables = Tables::from_env()?;
        let config = aws_config::load_from_env().await;
        Ok(Db::new(Client::new(&config), tables))
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn tables(&self) -> &Tables {
        &self.tables
    }

    fn table(&self, table: TableKey) -> &str {
        match table {
            TableKey::Main => &self.tables.main,
            TableKey::Audit => &self.tables.audit,
        }
    }

    /// Reads an item with a strongly consistent read.
    pub async fn get(&self, key: &Key) -> Result<Option<Item>, DbError> {
        let out = self
            .client
            .get_item()
            .table_name(self.table(key.table))
            .set_key(Some(key.attributes()))
            .consistent_read(true)
            .send()
            .await
            .map_err(unavailable)?;
        Ok(out.item)
    }

    /// Reads items with strongly consistent reads, in the order of the keys; a missing item
    /// is left out.
    pub async fn get_many(&self, keys: &[Key]) -> Result<Vec<Item>, DbError> {
        let mut found: HashMap<(String, String), Item> = HashMap::new();
        for chunk in keys.chunks(100) {
            let mut pending: Vec<Item> = chunk.iter().map(Key::attributes).collect();
            let table = self.table(chunk[0].table).to_string();
            while !pending.is_empty() {
                let request = KeysAndAttributes::builder()
                    .set_keys(Some(std::mem::take(&mut pending)))
                    .consistent_read(true)
                    .build()
                    .map_err(|e| DbError::Invalid(e.to_string()))?;
                let out = self
                    .client
                    .batch_get_item()
                    .request_items(&table, request)
                    .send()
                    .await
                    .map_err(unavailable)?;
                for item in out
                    .responses
                    .unwrap_or_default()
                    .remove(&table)
                    .unwrap_or_default()
                {
                    let pk = get_s(&item, "pk").unwrap_or_default().to_string();
                    let sk = get_s(&item, "sk").unwrap_or_default().to_string();
                    found.insert((pk, sk), item);
                }
                if let Some(mut rest) = out.unprocessed_keys
                    && let Some(r) = rest.remove(&table)
                {
                    pending = r.keys;
                }
            }
        }
        Ok(keys
            .iter()
            .filter_map(|k| found.remove(&(k.pk.clone(), k.sk.clone())))
            .collect())
    }

    /// The items of a partition whose sort key starts with `prefix`, in the order of the sort
    /// key, with strongly consistent reads.
    pub async fn query(
        &self,
        table: TableKey,
        pk: &str,
        prefix: &str,
    ) -> Result<Vec<Item>, DbError> {
        let mut items = Vec::new();
        let mut start = None;
        loop {
            let out = self
                .client
                .query()
                .table_name(self.table(table))
                .key_condition_expression("#pk = :pk AND begins_with(#sk, :prefix)")
                .expression_attribute_names("#pk", "pk")
                .expression_attribute_names("#sk", "sk")
                .expression_attribute_values(":pk", s(pk))
                .expression_attribute_values(":prefix", s(prefix))
                .consistent_read(true)
                .set_exclusive_start_key(start)
                .send()
                .await
                .map_err(unavailable)?;
            items.extend(out.items.unwrap_or_default());
            match out.last_evaluated_key {
                Some(key) if !key.is_empty() => start = Some(key),
                _ => return Ok(items),
            }
        }
    }

    /// The unit of work of a User.
    pub fn tx(&self, user: &str) -> UserTx {
        UserTx::new(self.clone(), user)
    }
}

pub(crate) fn unavailable<E: ProvideErrorMetadata + std::fmt::Debug, R: std::fmt::Debug>(
    e: SdkError<E, R>,
) -> DbError {
    match e.as_service_error() {
        Some(service) => DbError::Unavailable(format!(
            "{}: {}",
            service.code().unwrap_or("unknown"),
            service.message().unwrap_or_default()
        )),
        None => DbError::Unavailable(format!("{e:?}")),
    }
}

/// The partition of a User.
pub fn user_pk(user: &str) -> String {
    format!("U#{user}")
}

/// The writes of a User, as one `TransactWriteItems`. A key that is not under `U#<user>` is
/// rejected, and so is a second action on the same item.
#[derive(Debug, Clone)]
pub struct UserTx {
    db: Db,
    user: String,
    actions: Vec<TransactWriteItem>,
    written: BTreeSet<Key>,
    bytes: usize,
}

impl UserTx {
    fn new(db: Db, user: &str) -> UserTx {
        UserTx {
            db,
            user: user.to_string(),
            actions: Vec::new(),
            written: BTreeSet::new(),
            bytes: 0,
        }
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    pub fn len(&self) -> usize {
        self.actions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    /// The bytes of the items the transaction writes.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Whether a key belongs to the User.
    pub fn owns(&self, key: &Key) -> bool {
        let pk = user_pk(&self.user);
        key.pk == pk || key.pk.starts_with(&format!("{pk}#"))
    }

    fn claim(&mut self, key: &Key, bytes: usize) -> Result<(), DbError> {
        if self.user.is_empty() || self.user.contains('#') || !self.owns(key) {
            return Err(DbError::ForeignKey {
                user: self.user.clone(),
                pk: key.pk.clone(),
                sk: key.sk.clone(),
            });
        }
        if !self.written.insert(key.clone()) {
            return Err(DbError::SameItem(format!("{} {}", key.pk, key.sk)));
        }
        if bytes > MAX_ITEM_BYTES {
            return Err(DbError::TooLarge(format!(
                "{} {} has {bytes} bytes",
                key.pk, key.sk
            )));
        }
        if self.actions.len() == MAX_ACTIONS {
            return Err(DbError::TooLarge(format!(
                "more than {MAX_ACTIONS} actions"
            )));
        }
        if self.bytes + bytes > MAX_TRANSACTION_BYTES {
            return Err(DbError::TooLarge(format!(
                "more than {MAX_TRANSACTION_BYTES} bytes"
            )));
        }
        self.bytes += bytes;
        Ok(())
    }

    fn key_of(item: &Item, table: TableKey) -> Result<Key, DbError> {
        let pk = get_s(item, "pk").ok_or_else(|| DbError::Invalid("an item has no pk".into()))?;
        let sk = get_s(item, "sk").ok_or_else(|| DbError::Invalid("an item has no sk".into()))?;
        Ok(Key {
            table,
            pk: pk.to_string(),
            sk: sk.to_string(),
        })
    }

    /// Writes an item, under a condition.
    pub fn put(
        &mut self,
        table: TableKey,
        item: Item,
        condition: Option<Cond>,
    ) -> Result<(), DbError> {
        let key = Self::key_of(&item, table)?;
        self.claim(&key, item_size(&item))?;
        let mut put = Put::builder()
            .table_name(self.db.table(table))
            .set_item(Some(item));
        if let Some(c) = condition {
            put = put
                .condition_expression(c.expression)
                .set_expression_attribute_names(Some(c.names))
                .set_expression_attribute_values(non_empty(c.values));
        }
        let put = put.build().map_err(|e| DbError::Invalid(e.to_string()))?;
        self.actions
            .push(TransactWriteItem::builder().put(put).build());
        Ok(())
    }

    /// Writes a new item; the transaction fails when the item exists.
    pub fn insert(&mut self, item: Item) -> Result<(), DbError> {
        self.put(TableKey::Main, item, Some(Cond::not_exists()))
    }

    pub fn update(&mut self, key: &Key, change: Change) -> Result<(), DbError> {
        let (expression, mut names, mut values) = change.expression();
        let bytes =
            key.pk.len() + key.sk.len() + values.values().map(attribute_size).sum::<usize>();
        self.claim(key, bytes)?;
        let mut update = Update::builder()
            .table_name(self.db.table(key.table))
            .set_key(Some(key.attributes()))
            .update_expression(expression);
        if let Some(c) = change.condition {
            names.extend(c.names);
            values.extend(c.values);
            update = update.condition_expression(c.expression);
        }
        let update = update
            .set_expression_attribute_names(Some(names))
            .set_expression_attribute_values(non_empty(values))
            .build()
            .map_err(|e| DbError::Invalid(e.to_string()))?;
        self.actions
            .push(TransactWriteItem::builder().update(update).build());
        Ok(())
    }

    pub fn delete(&mut self, key: &Key, condition: Option<Cond>) -> Result<(), DbError> {
        self.claim(key, key.pk.len() + key.sk.len())?;
        let mut delete = Delete::builder()
            .table_name(self.db.table(key.table))
            .set_key(Some(key.attributes()));
        if let Some(c) = condition {
            delete = delete
                .condition_expression(c.expression)
                .set_expression_attribute_names(Some(c.names))
                .set_expression_attribute_values(non_empty(c.values));
        }
        let delete = delete
            .build()
            .map_err(|e| DbError::Invalid(e.to_string()))?;
        self.actions
            .push(TransactWriteItem::builder().delete(delete).build());
        Ok(())
    }

    /// Requires a condition of an item that the transaction does not write.
    pub fn check(&mut self, key: &Key, condition: Cond) -> Result<(), DbError> {
        self.claim(key, key.pk.len() + key.sk.len())?;
        let check = ConditionCheck::builder()
            .table_name(self.db.table(key.table))
            .set_key(Some(key.attributes()))
            .condition_expression(condition.expression)
            .set_expression_attribute_names(Some(condition.names))
            .set_expression_attribute_values(non_empty(condition.values))
            .build()
            .map_err(|e| DbError::Invalid(e.to_string()))?;
        self.actions
            .push(TransactWriteItem::builder().condition_check(check).build());
        Ok(())
    }

    /// Writes every action at once, or none.
    pub async fn commit(self) -> Result<(), DbError> {
        if self.actions.is_empty() {
            return Ok(());
        }
        let result = self
            .db
            .client
            .transact_write_items()
            .set_transact_items(Some(self.actions))
            .send()
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(e) => {
                if let Some(service) = e.as_service_error()
                    && let aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError::TransactionCanceledException(c) = service
                {
                    let reasons = c.cancellation_reasons();
                    if let Some(index) = reasons
                        .iter()
                        .position(|r| r.code() == Some("ConditionalCheckFailed"))
                    {
                        return Err(DbError::Condition { index });
                    }
                    if reasons.iter().any(|r| r.code() == Some("TransactionConflict")) {
                        return Err(DbError::Conflict);
                    }
                    return Err(DbError::Unavailable(format!(
                        "the transaction was canceled: {:?}",
                        reasons.iter().filter_map(|r| r.code()).collect::<Vec<_>>()
                    )));
                }
                if e.as_service_error().and_then(|s| s.code())
                    == Some("TransactionConflictException")
                {
                    return Err(DbError::Conflict);
                }
                Err(unavailable(e))
            }
        }
    }
}

fn non_empty(values: HashMap<String, AttributeValue>) -> Option<HashMap<String, AttributeValue>> {
    (!values.is_empty()).then_some(values)
}
