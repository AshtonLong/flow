//! Lenient validation of a raw config value.
//!
//! The TypeScript validates with a zod schema and, while parsing fails, drops
//! every key zod complains about and tries again. This port keeps that loop and
//! replaces zod with a small interpreter of the same schema that reports the
//! same issues, in the same order, with the messages zod v4 prints.
//!
//! PORT: src/shared/config.ts `ConfigSchema`, `validateConfig`

use std::sync::OnceLock;

use serde_json::{Map, Value};

use super::{CONFIG_VERSION, Config, ConfigIssue, DEFAULT_LOCAL_MODEL, PathSeg};

/// A validated config and the problems found on the way.
#[derive(Debug, Clone, PartialEq)]
pub struct Validated {
    pub config: Config,
    pub issues: Vec<ConfigIssue>,
}

/// The top-level keys of the schema (`ConfigSchema.shape`).
pub const CONFIG_KEYS: [&str; 15] = [
    "version",
    "general",
    "hotkeys",
    "audio",
    "model",
    "providers",
    "cleanup",
    "prompts",
    "insert",
    "overlay",
    "history",
    "dictionary",
    "snippets",
    "local_models",
    "profiles",
];

const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// One node of the schema, mirroring the zod types the TS uses.
enum Schema {
    Boolean,
    String { min: Option<usize>, url: bool, starts_with: Option<&'static str> },
    Number { int: bool, min: Option<f64>, max: Option<f64> },
    Enum(&'static [&'static str]),
    Literal(&'static str),
    Object(Vec<(&'static str, Field)>),
    Record(Box<Schema>),
    Array { item: Box<Schema>, min: Option<usize> },
    Unknown,
}

/// How an object key may be absent.
enum Presence {
    Required,
    /// `.optional()`: absent stays absent.
    Optional,
    /// `.default(value)`: absent becomes `value`, unvalidated.
    Default(Value),
    /// `.prefault(value)`: absent becomes `value`, which is then validated.
    Prefault(Value),
}

struct Field {
    schema: Schema,
    presence: Presence,
}

fn string() -> Schema {
    Schema::String { min: None, url: false, starts_with: None }
}

fn nonempty_string() -> Schema {
    Schema::String { min: Some(1), url: false, starts_with: None }
}

fn int(min: Option<f64>, max: Option<f64>) -> Schema {
    Schema::Number { int: true, min, max }
}

fn required(schema: Schema) -> Field {
    Field { schema, presence: Presence::Required }
}

fn optional(schema: Schema) -> Field {
    Field { schema, presence: Presence::Optional }
}

fn default(schema: Schema, value: impl Into<Value>) -> Field {
    Field { schema, presence: Presence::Default(value.into()) }
}

/// A table whose keys all have defaults, so the table itself may be omitted.
fn section(shape: Vec<(&'static str, Field)>) -> Field {
    Field { schema: Schema::Object(shape), presence: Presence::Prefault(Value::Object(Map::new())) }
}

/// Per-app override sections are loose here; the merged result is re-validated.
fn overrides() -> Field {
    optional(Schema::Record(Box::new(Schema::Unknown)))
}

fn config_schema() -> &'static Schema {
    static SCHEMA: OnceLock<Schema> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let general = section(vec![
            ("launch_at_login", default(Schema::Boolean, false)),
            ("theme", default(Schema::Enum(&["system", "light", "dark"]), "system")),
            ("language", default(Schema::Literal("en"), "en")),
            ("debug_logging", default(Schema::Boolean, false)),
            ("onboarded", default(Schema::Boolean, false)),
        ]);
        let hotkeys = section(vec![
            ("hold_to_talk", default(string(), "Ctrl+Win")),
            ("toggle", default(string(), "Ctrl+Win+Space")),
            ("cancel", default(string(), "Esc")),
            ("paste_last", default(string(), "Alt+Shift+V")),
            ("double_tap_lock", default(Schema::Boolean, true)),
        ]);
        let audio = section(vec![
            ("input_device", default(string(), "default")),
            ("keep_mic_warm", default(Schema::Boolean, false)),
            ("sounds", default(Schema::Boolean, true)),
            ("max_recording_seconds", default(int(Some(5.0), Some(3600.0)), 300)),
        ]);
        let model = section(vec![
            ("active", default(nonempty_string(), DEFAULT_LOCAL_MODEL)),
            ("fallback", default(nonempty_string(), DEFAULT_LOCAL_MODEL)),
            ("device", default(Schema::Enum(&["auto", "cpu", "gpu"]), "auto")),
            ("keep_loaded_minutes", default(Schema::Number { int: false, min: Some(-1.0), max: Some(1440.0) }, 10)),
        ]);
        let provider = Schema::Object(vec![
            // Required for custom endpoints; optional override for built-in providers.
            ("base_url", optional(Schema::String { min: None, url: true, starts_with: None })),
            // A reference such as `secret:groq`. The key itself is never stored here.
            ("api_key", optional(Schema::String { min: None, url: false, starts_with: Some("secret:") })),
            // Model name sent to a custom OpenAI-compatible endpoint.
            ("model", optional(string())),
        ]);
        let cleanup = section(vec![
            ("filler_removal", default(Schema::Boolean, true)),
            ("spoken_formatting", default(Schema::Boolean, true)),
            ("spoken_punctuation", default(Schema::Boolean, true)),
            ("llm_enabled", default(Schema::Boolean, false)),
            ("llm_model", default(string(), "groq/openai/gpt-oss-20b")),
            ("llm_prompt", default(string(), "clean")),
            ("llm_timeout_ms", default(int(Some(500.0), Some(30000.0)), 3000)),
        ]);
        let prompt = Schema::Object(vec![("text", required(string()))]);
        let insert = section(vec![
            ("method", default(Schema::Enum(&["paste", "type"]), "paste")),
            ("paste_shortcut", default(string(), "Ctrl+V")),
            ("restore_clipboard", default(Schema::Boolean, true)),
            ("trailing_space", default(Schema::Boolean, true)),
            ("match_case", default(Schema::Boolean, true)),
        ]);
        let overlay = section(vec![
            ("style", default(Schema::Enum(&["pill", "minimal", "none"]), "pill")),
            (
                "position",
                default(
                    Schema::Enum(&[
                        "bottom-center",
                        "bottom-left",
                        "bottom-right",
                        "top-center",
                        "top-left",
                        "top-right",
                    ]),
                    "bottom-center",
                ),
            ),
            ("size", default(Schema::Enum(&["small", "medium", "large"]), "medium")),
            ("show_waveform", default(Schema::Boolean, true)),
        ]);
        let history = section(vec![
            ("enabled", default(Schema::Boolean, false)),
            ("retention_days", default(int(Some(1.0), Some(3650.0)), 30)),
        ]);
        let dictionary_entry = Schema::Object(vec![
            ("heard", required(Schema::Array { item: Box::new(nonempty_string()), min: Some(1) })),
            ("write", required(nonempty_string())),
        ]);
        let snippet = Schema::Object(vec![("trigger", required(nonempty_string())), ("text", required(string()))]);
        let local_model = Schema::Object(vec![
            ("id", required(nonempty_string())),
            ("name", optional(string())),
            ("path", required(nonempty_string())),
        ]);
        let profile = Schema::Object(vec![
            ("name", required(nonempty_string())),
            ("match_process", default(Schema::Array { item: Box::new(string()), min: None }, Value::Array(vec![]))),
            ("match_title", default(Schema::Array { item: Box::new(string()), min: None }, Value::Array(vec![]))),
            // `false` disables dictation entirely while the app is focused.
            ("enabled", default(Schema::Boolean, true)),
            ("audio", overrides()),
            ("model", overrides()),
            ("cleanup", overrides()),
            ("insert", overrides()),
            ("overlay", overrides()),
            ("history", overrides()),
        ]);
        let list = |item: Schema| default(Schema::Array { item: Box::new(item), min: None }, Value::Array(vec![]));
        Schema::Object(vec![
            ("version", default(int(None, None), CONFIG_VERSION)),
            ("general", general),
            ("hotkeys", hotkeys),
            ("audio", audio),
            ("model", model),
            ("providers", default(Schema::Record(Box::new(provider)), Value::Object(Map::new()))),
            ("cleanup", cleanup),
            ("prompts", default(Schema::Record(Box::new(prompt)), Value::Object(Map::new()))),
            ("insert", insert),
            ("overlay", overlay),
            ("history", history),
            ("dictionary", list(dictionary_entry)),
            ("snippets", list(snippet)),
            ("local_models", list(local_model)),
            ("profiles", list(profile)),
        ])
    })
}

/// zod's name for the type of an input (`util.parsedType`); `None` is `undefined`.
fn type_name(input: Option<&Value>) -> &'static str {
    match input {
        None => "undefined",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}

fn invalid_type(expected: &str, input: Option<&Value>) -> String {
    format!("Invalid input: expected {expected}, received {}", type_name(input))
}

/// `util.joinValues(values, "|")` over strings.
fn join_values(values: &[&str]) -> String {
    values.iter().map(|value| format!("\"{value}\"")).collect::<Vec<_>>().join("|")
}

/// `number.toString()` for the bounds in messages.
fn bound(value: f64) -> String {
    super::toml_write::js_number_string(value)
}

/// The JavaScript `String.prototype.trim`.
fn js_trim(text: &str) -> &str {
    text.trim_matches(super::patch::is_js_whitespace)
}

/// A string's length in code points (zod counts code points for `min`).
fn length_of(text: &str) -> usize {
    text.chars().count()
}

struct Checker {
    issues: Vec<(Vec<PathSeg>, String)>,
    path: Vec<PathSeg>,
}

impl Checker {
    fn issue(&mut self, message: String) {
        self.issues.push((self.path.clone(), message));
    }

    /// Runs `schema` on `input` (`None` is `undefined`). Returns the parsed
    /// value; issues are collected in the order zod reports them.
    fn run(&mut self, schema: &Schema, input: Option<&Value>) -> Option<Value> {
        match schema {
            Schema::Unknown => input.cloned(),
            Schema::Boolean => match input {
                Some(Value::Bool(b)) => Some(Value::Bool(*b)),
                other => {
                    self.issue(invalid_type("boolean", other));
                    None
                }
            },
            Schema::String { min, url, starts_with } => {
                let Some(Value::String(text)) = input else {
                    self.issue(invalid_type("string", input));
                    // A length check still runs on anything with a length; its issue
                    // has the same path, so validation never reports it.
                    return None;
                };
                let mut out = text.clone();
                if let Some(min) = min
                    && length_of(text) < *min
                {
                    self.issue(format!("Too small: expected string to have >={min} characters"));
                }
                if *url {
                    let trimmed = js_trim(text);
                    if url::Url::parse(trimmed).is_ok() {
                        out = trimmed.chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect();
                    } else {
                        self.issue("Invalid URL".into());
                    }
                }
                if let Some(prefix) = starts_with
                    && !text.starts_with(prefix)
                {
                    self.issue(format!("Invalid string: must start with \"{prefix}\""));
                }
                Some(Value::String(out))
            }
            Schema::Number { int, min, max } => {
                let Some(Value::Number(number)) = input else {
                    self.issue(invalid_type("number", input));
                    return None;
                };
                let value = number.as_f64().unwrap_or(f64::NAN);
                if *int {
                    if value.fract() != 0.0 {
                        self.issue("Invalid input: expected int, received number".into());
                        return None;
                    }
                    if value.abs() > MAX_SAFE_INTEGER {
                        if value > 0.0 {
                            self.issue(format!("Too big: expected int to be <={}", bound(MAX_SAFE_INTEGER)));
                        } else {
                            self.issue(format!("Too small: expected int to be >={}", bound(-MAX_SAFE_INTEGER)));
                        }
                    }
                }
                if let Some(min) = min
                    && value < *min
                {
                    self.issue(format!("Too small: expected number to be >={}", bound(*min)));
                }
                if let Some(max) = max
                    && value > *max
                {
                    self.issue(format!("Too big: expected number to be <={}", bound(*max)));
                }
                if *int && value.abs() <= MAX_SAFE_INTEGER {
                    Some(Value::from(value as i64))
                } else {
                    Some(Value::Number(number.clone()))
                }
            }
            Schema::Enum(values) => match input {
                Some(Value::String(text)) if values.contains(&text.as_str()) => Some(Value::String(text.clone())),
                _ => {
                    self.issue(format!("Invalid option: expected one of {}", join_values(values)));
                    None
                }
            },
            Schema::Literal(literal) => match input {
                Some(Value::String(text)) if text == literal => Some(Value::String(text.clone())),
                _ => {
                    self.issue(format!("Invalid input: expected \"{literal}\""));
                    None
                }
            },
            Schema::Object(shape) => {
                let Some(Value::Object(map)) = input else {
                    self.issue(invalid_type("object", input));
                    return None;
                };
                let mut out = Map::new();
                for (key, field) in shape {
                    self.path.push(PathSeg::from(*key));
                    if let Some(value) = self.run_field(field, map.get(*key)) {
                        out.insert((*key).to_string(), value);
                    }
                    self.path.pop();
                }
                // Unknown keys are stripped silently.
                Some(Value::Object(out))
            }
            Schema::Record(value_schema) => {
                let Some(Value::Object(map)) = input else {
                    self.issue(invalid_type("record", input));
                    return None;
                };
                let mut out = Map::new();
                for key in js_key_order(map) {
                    if key == "__proto__" {
                        continue;
                    }
                    self.path.push(PathSeg::Key(key.clone()));
                    if let Some(value) = self.run(value_schema, map.get(key)) {
                        out.insert(key.clone(), value);
                    }
                    self.path.pop();
                }
                Some(Value::Object(out))
            }
            Schema::Array { item, min } => {
                let Some(Value::Array(items)) = input else {
                    self.issue(invalid_type("array", input));
                    return None;
                };
                let mut out = Vec::with_capacity(items.len());
                for (index, value) in items.iter().enumerate() {
                    self.path.push(PathSeg::Index(index));
                    out.push(self.run(item, Some(value)).unwrap_or(Value::Null));
                    self.path.pop();
                }
                if let Some(min) = min
                    && items.len() < *min
                {
                    self.issue(format!("Too small: expected array to have >={min} items"));
                }
                Some(Value::Array(out))
            }
        }
    }

    /// One key of an object, with zod's handling of absent keys.
    fn run_field(&mut self, field: &Field, input: Option<&Value>) -> Option<Value> {
        match (&field.presence, input) {
            (Presence::Optional, None) => None,
            (Presence::Default(value), None) => Some(value.clone()),
            (Presence::Prefault(value), None) => self.run(&field.schema, Some(value)),
            (_, input) => self.run(&field.schema, input),
        }
    }
}

/// Object keys in JavaScript order: integer-like keys first, ascending, then
/// the rest in insertion order.
fn js_key_order(map: &Map<String, Value>) -> Vec<&String> {
    let index = |key: &str| -> Option<u32> {
        let n: u32 = key.parse().ok()?;
        (n != u32::MAX && n.to_string() == key).then_some(n)
    };
    let mut numeric: Vec<(u32, &String)> = map.keys().filter_map(|key| index(key).map(|n| (n, key))).collect();
    numeric.sort_by_key(|(n, _)| *n);
    let mut keys: Vec<&String> = numeric.into_iter().map(|(_, key)| key).collect();
    keys.extend(map.keys().filter(|key| index(key).is_none()));
    keys
}

/// `ConfigSchema.safeParse`: the config, or every issue zod would report.
fn safe_parse(raw: &Value) -> Result<Config, Vec<(Vec<PathSeg>, String)>> {
    let mut checker = Checker { issues: Vec::new(), path: Vec::new() };
    let out = checker.run(config_schema(), Some(raw));
    if !checker.issues.is_empty() {
        return Err(checker.issues);
    }
    match out.map(serde_json::from_value::<Config>) {
        Some(Ok(config)) => Ok(config),
        // The schema and `Config` agree, so this does not happen.
        Some(Err(error)) => Err(vec![(Vec::new(), error.to_string())]),
        None => Err(vec![(Vec::new(), invalid_type("object", Some(raw)))]),
    }
}

/// `deleteAt` from the TS: removes the key or list item at `path`. Returns
/// whether something was removed.
fn delete_at(root: &mut Value, path: &[PathSeg]) -> bool {
    let Some((last, parents)) = path.split_last() else { return false };
    let mut node = root;
    for seg in parents {
        let next = match (node, seg) {
            (Value::Object(map), PathSeg::Key(key)) => map.get_mut(key),
            (Value::Object(map), PathSeg::Index(index)) => map.get_mut(&index.to_string()),
            (Value::Array(items), PathSeg::Index(index)) => items.get_mut(*index),
            _ => None,
        };
        match next {
            Some(next @ (Value::Object(_) | Value::Array(_))) => node = next,
            _ => return false,
        }
    }
    match (node, last) {
        (Value::Array(items), PathSeg::Index(index)) => {
            if *index >= items.len() {
                return false;
            }
            items.remove(*index);
            true
        }
        (Value::Object(map), PathSeg::Key(key)) => map.shift_remove(key).is_some(),
        (Value::Object(map), PathSeg::Index(index)) => map.shift_remove(&index.to_string()).is_some(),
        _ => false,
    }
}

/// The issue sort order: deepest paths and highest array indexes first, so
/// removals don't shift siblings. Not a total order (as in the TS), so the
/// sort below reproduces V8's exactly.
fn compare(a: &[PathSeg], b: &[PathSeg]) -> i64 {
    if a.len() != b.len() {
        return b.len() as i64 - a.len() as i64;
    }
    match (a.last(), b.last()) {
        (Some(PathSeg::Index(al)), Some(PathSeg::Index(bl))) => *bl as i64 - *al as i64,
        _ => 0,
    }
}

/// `Array.prototype.sort` as V8 runs it on short arrays (TimSort's initial
/// run plus binary insertion sort), so an inconsistent comparator orders the
/// same way it does in the TypeScript.
fn v8_sort<T>(items: &mut [T], cmp: impl Fn(&T, &T) -> i64) {
    let n = items.len();
    if n < 2 {
        return;
    }
    // CountAndMakeRun.
    let mut run = 2;
    let descending = cmp(&items[1], &items[0]) < 0;
    let mut previous = 1;
    while run < n {
        let order = cmp(&items[run], &items[previous]);
        if (descending && order >= 0) || (!descending && order < 0) {
            break;
        }
        previous = run;
        run += 1;
    }
    if descending {
        items[..run].reverse();
    }
    // BinaryInsertionSort from the end of the run.
    for start in run..n {
        let (mut left, mut right) = (0, start);
        while left < right {
            let mid = left + ((right - left) >> 1);
            if cmp(&items[start], &items[mid]) < 0 {
                right = mid;
            } else {
                left = mid + 1;
            }
        }
        items[left..=start].rotate_right(1);
    }
}

/// The list item a bad field belongs to: an invalid field inside a list item
/// drops the whole item.
fn deletion_target(path: &[PathSeg]) -> &[PathSeg] {
    let top = match path.first() {
        Some(PathSeg::Key(key)) => key.as_str(),
        _ => "",
    };
    if matches!(top, "dictionary" | "snippets" | "profiles" | "local_models") && path.len() > 2 {
        &path[..2]
    } else {
        path
    }
}

/// Validates leniently: a bad key is dropped (so it falls back to its default)
/// and reported, instead of rejecting the whole file. Never panics.
pub fn validate_config(raw: &Value) -> Validated {
    let mut issues: Vec<ConfigIssue> = Vec::new();
    if !raw.is_object() {
        return Validated { config: Config::default(), issues };
    }
    let mut work = raw.clone();
    for _attempt in 0..200 {
        let found = match safe_parse(&work) {
            Ok(config) => return Validated { config, issues },
            Err(found) => found,
        };
        let mut removed = false;
        let mut sorted = found;
        v8_sort(&mut sorted, |a, b| compare(&a.0, &b.0));
        let mut seen: Vec<Vec<PathSeg>> = Vec::new();
        for (path, message) in sorted {
            if seen.contains(&path) {
                continue;
            }
            seen.push(path.clone());
            if delete_at(&mut work, deletion_target(&path)) {
                removed = true;
            }
            issues.push(ConfigIssue { path, message, line: None });
        }
        if !removed {
            break;
        }
    }
    Validated { config: Config::default(), issues }
}
