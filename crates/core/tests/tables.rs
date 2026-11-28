use serde_json::Value;

const TABLES: &str = include_str!("../../../infra/tables.json");

fn tables() -> Vec<Value> {
    let root: Value = serde_json::from_str(TABLES).expect("infra/tables.json is JSON");
    root["tables"].as_array().expect("a list of tables").clone()
}

fn table(role: &str) -> Value {
    tables()
        .into_iter()
        .find(|t| t["role"] == role)
        .unwrap_or_else(|| panic!("no {role} table"))
}

fn key_schema(value: &Value) -> Vec<(String, String)> {
    value
        .as_array()
        .expect("a key schema")
        .iter()
        .map(|k| {
            (
                k["AttributeName"].as_str().unwrap().to_string(),
                k["KeyType"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn pair(hash: &str, range: &str) -> Vec<(String, String)> {
    vec![
        (hash.to_string(), "HASH".to_string()),
        (range.to_string(), "RANGE".to_string()),
    ]
}

fn defined_attributes(table: &Value) -> Vec<String> {
    table["properties"]["AttributeDefinitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            assert_eq!(a["AttributeType"], "S");
            a["AttributeName"].as_str().unwrap().to_string()
        })
        .collect()
}

#[test]
fn there_are_the_main_and_audit_tables() {
    let mut roles: Vec<String> = tables()
        .iter()
        .map(|t| t["role"].as_str().unwrap().to_string())
        .collect();
    roles.sort();
    assert_eq!(roles, ["audit", "main"]);
}

#[test]
fn main_is_keyed_by_pk_and_sk_with_gsi1_and_gsi2() {
    let main = table("main");
    let properties = &main["properties"];
    assert_eq!(key_schema(&properties["KeySchema"]), pair("pk", "sk"));
    let indexes = properties["GlobalSecondaryIndexes"].as_array().unwrap();
    let names: Vec<&str> = indexes
        .iter()
        .map(|i| i["IndexName"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["gsi1", "gsi2"]);
    for index in indexes {
        let name = index["IndexName"].as_str().unwrap();
        assert_eq!(
            key_schema(&index["KeySchema"]),
            pair(&format!("{name}pk"), &format!("{name}sk"))
        );
    }
    let mut attributes = defined_attributes(&main);
    attributes.sort();
    assert_eq!(
        attributes,
        ["gsi1pk", "gsi1sk", "gsi2pk", "gsi2sk", "pk", "sk"]
    );
    assert_eq!(
        properties["StreamSpecification"]["StreamViewType"],
        "NEW_IMAGE"
    );
    assert_eq!(properties["TimeToLiveSpecification"]["Enabled"], true);
}

#[test]
fn audit_is_append_only_and_keyed_by_pk_and_sk() {
    let audit = table("audit");
    assert_eq!(audit["appendOnly"], true);
    assert_eq!(
        key_schema(&audit["properties"]["KeySchema"]),
        pair("pk", "sk")
    );
    assert_eq!(defined_attributes(&audit), ["pk", "sk"]);
    let root: Value = serde_json::from_str(TABLES).unwrap();
    let denied: Vec<&str> = root["appendOnlyDeniedActions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    for action in ["dynamodb:UpdateItem", "dynamodb:DeleteItem"] {
        assert!(denied.contains(&action), "{action}");
    }
}
