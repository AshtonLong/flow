//! Config file migrations. Each step takes the parsed file at one version and
//! returns it at the next; the store writes the result back as in-place patches
//! so comments survive a migration too.
//!
//! PORT: src/main/config/migrations.ts

use serde_json::{Map, Value};

use super::{ConfigPatch, PathSeg};

/// A parsed config file, as plain JSON (the TS `PlainObject`).
pub type PlainObject = Map<String, Value>;

#[derive(Clone, Copy)]
pub struct Migration {
    pub from: i64,
    pub to: i64,
    /// Receives a private copy of the parsed file and returns it at version
    /// `to`, or the reason it cannot.
    pub migrate: fn(PlainObject) -> Result<PlainObject, String>,
}

impl std::fmt::Debug for Migration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Migration").field("from", &self.from).field("to", &self.to).finish_non_exhaustive()
    }
}

/// Ordered list of migrations. Version 1 is the first format, so there are none yet.
pub const MIGRATIONS: &[Migration] = &[];

/// A JSON number as a JavaScript number.
fn js_number(value: &Value) -> Option<f64> {
    value.as_f64()
}

/// The version a parsed file is at. A file with no `version` key counts as the
/// first version, so later migrations still reach it; `None` means the value is
/// not a usable version number. Versions are JavaScript numbers, hence `f64`.
pub fn file_version(raw: &PlainObject) -> Option<f64> {
    let Some(version) = raw.get("version") else { return Some(1.0) };
    let number = js_number(version)?;
    (number.is_finite() && number.fract() == 0.0 && number >= 1.0).then_some(number)
}

/// Runs every applicable migration in order, from `version` towards `target`.
/// Stops early if a step is missing. Fails with whatever a migration fails with.
pub fn run_migrations(
    raw: &PlainObject,
    version: f64,
    target: i64,
    migrations: &[Migration],
) -> Result<(PlainObject, f64), String> {
    let mut current = raw.clone();
    let mut reached = version;
    while reached < target as f64 {
        let Some(step) = migrations.iter().find(|migration| migration.from as f64 == reached) else { break };
        if step.to as f64 <= reached {
            break;
        }
        current = (step.migrate)(current)?;
        reached = step.to as f64;
    }
    if reached != version {
        current.insert("version".into(), Value::from(reached as i64));
    }
    Ok((current, reached))
}

/// Structural equality with JavaScript number semantics (1 and 1.0 are equal).
pub(crate) fn json_equal(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => match (a, b) {
            (Value::Number(x), Value::Number(y)) => match (x.as_i64(), y.as_i64()) {
                (Some(x), Some(y)) => x == y,
                _ => {
                    let (x, y) = (x.as_f64().unwrap_or(f64::NAN), y.as_f64().unwrap_or(f64::NAN));
                    x == y && x.is_sign_negative() == y.is_sign_negative()
                }
            },
            (Value::Array(x), Value::Array(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(p, q)| json_equal(Some(p), Some(q)))
            }
            (Value::Object(x), Value::Object(y)) => {
                x.len() == y.len() && x.iter().all(|(key, value)| json_equal(Some(value), y.get(key)))
            }
            _ => a == b,
        },
        _ => false,
    }
}

/// The patches that turn `before` into `after`. Tables are compared key by key.
pub fn diff_patches(before: &PlainObject, after: &PlainObject, base: &[PathSeg]) -> Vec<ConfigPatch> {
    let mut patches = Vec::new();
    let keys = before.keys().chain(after.keys().filter(|key| !before.contains_key(*key)));
    for key in keys {
        let mut path = base.to_vec();
        path.push(PathSeg::Key(key.clone()));
        let (a, b) = (before.get(key), after.get(key));
        if json_equal(a, b) {
            continue;
        }
        match (a, b) {
            (Some(Value::Object(a)), Some(Value::Object(b))) => patches.extend(diff_patches(a, b, &path)),
            _ => patches.push(ConfigPatch { path, value: b.cloned() }),
        }
    }
    patches
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path;
    use serde_json::json;

    fn object(value: Value) -> PlainObject {
        value.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn reads_the_file_version() {
        assert_eq!(file_version(&object(json!({}))), Some(1.0));
        assert_eq!(file_version(&object(json!({ "version": 3 }))), Some(3.0));
        assert_eq!(file_version(&object(json!({ "version": 2.0 }))), Some(2.0));
        assert_eq!(file_version(&object(json!({ "version": 0 }))), None);
        assert_eq!(file_version(&object(json!({ "version": 1.5 }))), None);
        assert_eq!(file_version(&object(json!({ "version": "1" }))), None);
    }

    #[test]
    fn diffs_key_by_key() {
        let before = object(json!({ "version": 1, "audio": { "sounds": true, "x": 1 }, "gone": 1 }));
        let after = object(json!({ "version": 2, "audio": { "x": 1.0, "feedback": true } }));
        assert_eq!(
            diff_patches(&before, &after, &[]),
            vec![
                ConfigPatch::set(path!["version"], 2),
                ConfigPatch::remove(path!["audio", "sounds"]),
                ConfigPatch::set(path!["audio", "feedback"], true),
                ConfigPatch::remove(path!["gone"]),
            ]
        );
    }

    #[test]
    fn stops_at_a_missing_step() {
        fn up(mut raw: PlainObject) -> Result<PlainObject, String> {
            raw.insert("up".into(), json!(true));
            Ok(raw)
        }
        let steps = [Migration { from: 1, to: 2, migrate: up }];
        let (raw, reached) = run_migrations(&object(json!({})), 1.0, 3, &steps).unwrap();
        assert_eq!(reached, 2.0);
        assert_eq!(Value::Object(raw), json!({ "up": true, "version": 2 }));
    }
}
