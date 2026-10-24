//! Helpers over plain JSON values, the shape config patches and validation
//! operate on. Mirrors `applyPatches` / `formatPath` in
//! `src/renderer/settings/lib/patch.ts`.

use serde_json::{Map, Value};

use super::{ConfigPatch, PathSeg};

/// Returns a patched deep copy. `value: None` removes the key (or splices an
/// array item); missing containers on the way are created (an array when the
/// next segment is an index, else a table).
pub fn apply_patches(root: &Value, patches: &[ConfigPatch]) -> Value {
    let mut next = root.clone();
    for patch in patches {
        apply_one(&mut next, &patch.path, patch.value.as_ref());
    }
    next
}

fn apply_one(root: &mut Value, path: &[PathSeg], value: Option<&Value>) {
    let Some((last, parents)) = path.split_last() else { return };
    let mut node = root;
    for (i, seg) in parents.iter().enumerate() {
        let next_is_index = matches!(path[i + 1], PathSeg::Index(_));
        let child = match (node, seg) {
            (Value::Object(map), PathSeg::Key(key)) => {
                let exists = map.get(key).is_some_and(|v| v.is_object() || v.is_array());
                if !exists {
                    if value.is_none() {
                        return;
                    }
                    map.insert(
                        key.clone(),
                        if next_is_index { Value::Array(vec![]) } else { Value::Object(Map::new()) },
                    );
                }
                map.get_mut(key).unwrap()
            }
            (Value::Array(items), PathSeg::Index(index)) => {
                let exists = items.get(*index).is_some_and(|v| v.is_object() || v.is_array());
                if !exists {
                    if value.is_none() || *index > items.len() {
                        return;
                    }
                    let fresh = if next_is_index { Value::Array(vec![]) } else { Value::Object(Map::new()) };
                    if *index == items.len() {
                        items.push(fresh);
                    } else {
                        items[*index] = fresh;
                    }
                }
                &mut items[*index]
            }
            // A key into an array or an index into a table: nothing sensible to do.
            _ => return,
        };
        node = child;
    }
    match (node, last, value) {
        (Value::Object(map), PathSeg::Key(key), None) => {
            map.shift_remove(key);
        }
        (Value::Object(map), PathSeg::Key(key), Some(v)) => {
            map.insert(key.clone(), v.clone());
        }
        (Value::Array(items), PathSeg::Index(index), None) => {
            if *index < items.len() {
                items.remove(*index);
            }
        }
        (Value::Array(items), PathSeg::Index(index), Some(v)) => {
            if *index < items.len() {
                items[*index] = v.clone();
            } else if *index == items.len() {
                items.push(v.clone());
            }
        }
        _ => {}
    }
}

/// The value at a path, if any.
pub fn get_at<'a>(root: &'a Value, path: &[PathSeg]) -> Option<&'a Value> {
    let mut node = root;
    for seg in path {
        node = match (node, seg) {
            (Value::Object(map), PathSeg::Key(key)) => map.get(key)?,
            (Value::Array(items), PathSeg::Index(index)) => items.get(*index)?,
            _ => return None,
        };
    }
    Some(node)
}

/// `["audio", "max_recording_seconds"]` → `audio.max_recording_seconds`; indexes as `[n]`.
pub fn format_path(path: &[PathSeg]) -> String {
    let mut out = String::new();
    for seg in path {
        match seg {
            PathSeg::Index(i) => {
                out.push('[');
                out.push_str(&i.to_string());
                out.push(']');
            }
            PathSeg::Key(k) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(k);
            }
        }
    }
    out
}

/// Converts a parsed TOML value into the equivalent JSON value. Dates become
/// their TOML text, which the schema then rejects where a date is not allowed.
pub fn toml_to_json(value: toml::Value) -> Value {
    match value {
        toml::Value::String(s) => Value::String(s),
        toml::Value::Integer(i) => Value::from(i),
        toml::Value::Float(f) => serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null),
        toml::Value::Boolean(b) => Value::Bool(b),
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(items) => Value::Array(items.into_iter().map(toml_to_json).collect()),
        toml::Value::Table(table) => Value::Object(table.into_iter().map(|(k, v)| (k, toml_to_json(v))).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path;
    use serde_json::json;

    #[test]
    fn sets_creates_and_removes() {
        let root = json!({ "audio": { "sounds": true }, "dictionary": [{ "heard": ["a"], "write": "A" }] });
        let next = apply_patches(
            &root,
            &[
                ConfigPatch::set(path!["audio", "sounds"], false),
                ConfigPatch::set(path!["providers", "groq", "api_key"], "secret:groq"),
                ConfigPatch::remove(path!["dictionary", 0usize]),
                ConfigPatch::set(path!["snippets", 0usize, "trigger"], "sig"),
            ],
        );
        assert_eq!(
            next,
            json!({
                "audio": { "sounds": false },
                "dictionary": [],
                "providers": { "groq": { "api_key": "secret:groq" } },
                "snippets": [{ "trigger": "sig" }]
            })
        );
        // The input is untouched.
        assert_eq!(root["audio"]["sounds"], json!(true));
    }

    #[test]
    fn removing_a_missing_path_is_a_no_op() {
        let root = json!({ "a": 1 });
        assert_eq!(apply_patches(&root, &[ConfigPatch::remove(path!["b", "c"])]), root);
    }

    #[test]
    fn formats_paths() {
        assert_eq!(format_path(&path!["audio", "max_recording_seconds"]), "audio.max_recording_seconds");
        assert_eq!(format_path(&path!["dictionary", 2usize, "heard"]), "dictionary[2].heard");
    }

    #[test]
    fn gets_values() {
        let root = json!({ "a": [{ "b": 3 }] });
        assert_eq!(get_at(&root, &path!["a", 0usize, "b"]), Some(&json!(3)));
        assert_eq!(get_at(&root, &path!["a", 1usize]), None);
    }
}
