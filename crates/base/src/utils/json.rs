use deno_core::serde_json::Value;

pub fn merge_object(a: &mut Value, b: &Value) {
  match (a, b) {
    (Value::Object(a), Value::Object(b)) => {
      for (k, v) in b {
        merge_object(a.entry(k.clone()).or_insert(Value::Null), v);
      }
    }
    (a, b) => *a = b.clone(),
  }
}

#[cfg(test)]
mod test {
  use super::*;
  use deno_core::serde_json::json;

  #[test]
  fn merge_disjoint_objects() {
    let mut a = json!({"a": 1});
    let b = json!({"b": 2});
    merge_object(&mut a, &b);
    assert_eq!(a, json!({"a": 1, "b": 2}));
  }

  #[test]
  fn merge_overlapping_objects_b_wins() {
    let mut a = json!({"key": "old"});
    let b = json!({"key": "new"});
    merge_object(&mut a, &b);
    assert_eq!(a["key"], "new");
  }

  #[test]
  fn merge_nested_objects() {
    let mut a = json!({"outer": {"a": 1}});
    let b = json!({"outer": {"b": 2}});
    merge_object(&mut a, &b);
    assert_eq!(a, json!({"outer": {"a": 1, "b": 2}}));
  }

  #[test]
  fn merge_non_object_replaces() {
    let mut a = json!("string");
    let b = json!(42);
    merge_object(&mut a, &b);
    assert_eq!(a, json!(42));
  }

  #[test]
  fn merge_into_empty_object() {
    let mut a = json!({});
    let b = json!({"key": "value"});
    merge_object(&mut a, &b);
    assert_eq!(a, json!({"key": "value"}));
  }

  #[test]
  fn merge_with_null_values() {
    let mut a = json!({"key": "value"});
    let b = json!({"key": null});
    merge_object(&mut a, &b);
    assert_eq!(a["key"], Value::Null);
  }

  #[test]
  fn merge_deep_nested() {
    let mut a = json!({"l1": {"l2": {"l3": "a"}}});
    let b = json!({"l1": {"l2": {"l4": "b"}}});
    merge_object(&mut a, &b);
    assert_eq!(a["l1"]["l2"]["l3"], "a");
    assert_eq!(a["l1"]["l2"]["l4"], "b");
  }
}
