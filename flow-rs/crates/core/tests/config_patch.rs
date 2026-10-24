//! PORT: tests/unit/config/patch.test.ts

use std::collections::HashSet;

use flow_core::config::toml_write::deep_equal;
use flow_core::config::value::toml_to_json;
use flow_core::config::{
    Config, ConfigPatch, ConfigPath, PathSeg, TomlPatchError, parse_toml, patch_toml, validate_config,
};
use flow_core::path;
use serde_json::{Value as Json, json};
use toml::Value;

/// The example from SPEC.md "Configuration", comments included.
fn example() -> String {
    include_str!("fixtures/example.toml").replace("\r\n", "\n")
}

type Table = toml::Table;

const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

fn set(path: ConfigPath, value: Json) -> ConfigPatch {
    ConfigPatch { path, value: Some(value) }
}

fn remove(path: ConfigPath) -> ConfigPatch {
    ConfigPatch { path, value: None }
}

/// `String.prototype.replace` with a string pattern: the first occurrence only.
fn rep(text: &str, from: &str, to: &str) -> String {
    assert!(text.contains(from), "{from:?} not found");
    text.replacen(from, to, 1)
}

fn parse(text: &str) -> Value {
    match parse_toml(text) {
        Ok(table) => Value::Table(table),
        Err(error) => panic!("{error}\n--- text ---\n{text}"),
    }
}

/// A patch value as the JavaScript reference implementation holds it: a number
/// is just a number, so an integral one reads back as an integer.
fn to_toml(value: &Json) -> Option<Value> {
    match value {
        Json::Null => None,
        Json::Bool(b) => Some(Value::Boolean(*b)),
        Json::Number(n) => {
            let f = n.as_f64().unwrap_or(f64::NAN);
            Some(match n.as_i64() {
                Some(i) if (i as f64).abs() <= MAX_SAFE_INTEGER => Value::Integer(i),
                _ if f.fract() == 0.0 && f.abs() <= MAX_SAFE_INTEGER => Value::Integer(f as i64),
                _ => Value::Float(f),
            })
        }
        Json::String(s) => Some(Value::String(s.clone())),
        Json::Array(items) => Some(Value::Array(items.iter().filter_map(to_toml).collect())),
        Json::Object(map) => {
            Some(Value::Table(map.iter().filter_map(|(k, v)| to_toml(v).map(|v| (k.clone(), v))).collect()))
        }
    }
}

fn key_text(seg: &PathSeg) -> String {
    seg.to_string()
}

fn child<'a>(node: &'a Value, seg: &PathSeg) -> Option<&'a Value> {
    match (node, seg) {
        (Value::Array(items), PathSeg::Index(i)) => items.get(*i),
        (Value::Table(table), seg) => table.get(&key_text(seg)),
        _ => None,
    }
}

fn child_mut<'a>(node: &'a mut Value, seg: &PathSeg) -> Option<&'a mut Value> {
    match (node, seg) {
        (Value::Array(items), PathSeg::Index(i)) => items.get_mut(*i),
        (Value::Table(table), seg) => table.get_mut(&key_text(seg)),
        _ => None,
    }
}

fn set_child(node: &mut Value, seg: &PathSeg, value: Value) {
    match (node, seg) {
        (Value::Array(items), PathSeg::Index(i)) => {
            if *i < items.len() {
                items[*i] = value;
            } else {
                items.push(value);
            }
        }
        (Value::Table(table), seg) => {
            table.insert(key_text(seg), value);
        }
        _ => {}
    }
}

/// Reference implementation of a patch, on the parsed structure.
fn apply_to_object(root: &mut Value, patch: &ConfigPatch) {
    let path = &patch.path;
    let value = patch.value.as_ref().and_then(to_toml);
    let mut node = root;
    for i in 0..path.len() - 1 {
        let part = &path[i];
        let is_object = matches!(child(node, part), Some(Value::Table(_) | Value::Array(_)));
        if !is_object {
            if value.is_none() {
                return;
            }
            let fresh = if matches!(path[i + 1], PathSeg::Index(_)) {
                Value::Array(vec![])
            } else {
                Value::Table(Table::new())
            };
            set_child(node, part, fresh);
        }
        node = child_mut(node, part).expect("just created");
    }
    let last = &path[path.len() - 1];
    match value {
        Some(value) => set_child(node, last, value),
        None => match (node, last) {
            (Value::Array(items), PathSeg::Index(i)) => {
                if *i < items.len() {
                    items.remove(*i);
                }
            }
            (Value::Table(table), seg) => {
                table.remove(&key_text(seg));
            }
            _ => {}
        },
    }
}

fn apply_all(source: &str, patches: &[ConfigPatch]) -> Value {
    let mut root = parse(source);
    for patch in patches {
        apply_to_object(&mut root, patch);
    }
    root
}

/// An empty table or list reads the same as an absent one.
fn prune(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(prune).collect()),
        Value::Table(table) => {
            let mut out = Table::new();
            for (key, child) in table {
                let pruned = prune(child);
                match &pruned {
                    Value::Array(items) if items.is_empty() => continue,
                    Value::Table(table) if table.is_empty() => continue,
                    _ => {}
                }
                out.insert(key.clone(), pruned);
            }
            Value::Table(out)
        }
        other => other.clone(),
    }
}

#[track_caller]
fn assert_same(actual: &Value, expected: &Value, context: &str) {
    assert!(deep_equal(actual, expected), "{context}\n  actual: {actual:?}\nexpected: {expected:?}");
}

/// Lines split like `text.split(/\r?\n/)`.
fn split_lines(text: &str) -> Vec<&str> {
    text.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line)).collect()
}

/// The comment part of every line that has one (no fixture has `#` inside a string).
fn comments(text: &str, marker: &str) -> Vec<String> {
    split_lines(text)
        .into_iter()
        .filter(|line| line.contains(marker))
        .map(|line| {
            let from = match line.rfind('#') {
                Some(i) => i,
                // `slice(-1)`: the last character.
                None => line.char_indices().last().map_or(0, |(i, _)| i),
            };
            line[from..].trim_end().to_string()
        })
        .collect()
}

#[track_caller]
fn expect_comments_kept(before: &str, after: &str, marker: &str) {
    let kept = comments(after, marker);
    for comment in comments(before, marker) {
        assert!(kept.contains(&comment), "lost comment {comment:?}\n--- text ---\n{after}");
    }
}

/// Patches, then checks the result parses to "old parse + patches applied".
#[track_caller]
fn patch_and_check(source: &str, patches: &[ConfigPatch]) -> String {
    let out = match patch_toml(source, patches) {
        Ok(out) => out,
        Err(error) => panic!("{error}"),
    };
    assert_same(&parse(&out), &apply_all(source, patches), &format!("--- out ---\n{out}"));
    out
}

fn lines(text: &str) -> Vec<&str> {
    text.split('\n').collect()
}

fn get<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(value, |node, key| node.get(*key))
}

mod existing_values {
    use super::*;

    #[test]
    fn replaces_only_the_value_token_of_a_key_in_a_table() {
        let example = example();
        let out = patch_and_check(&example, &[set(path!["general", "theme"], json!("dark"))]);
        assert_eq!(
            out,
            rep(
                &example,
                "theme = \"system\"              # system | light | dark",
                "theme = \"dark\"              # system | light | dark"
            )
        );
    }

    #[test]
    fn replaces_booleans_and_numbers_and_keeps_the_trailing_comment() {
        let example = example();
        let out = patch_and_check(
            &example,
            &[
                set(path!["audio", "keep_mic_warm"], json!(true)),
                set(path!["model", "keep_loaded_minutes"], json!(-1)),
                set(path!["audio", "max_recording_seconds"], json!(120)),
            ],
        );
        assert!(out.contains("keep_mic_warm = true         # true = faster start, mic indicator stays on"));
        assert!(out.contains("keep_loaded_minutes = -1      # 0 = unload at once, -1 = never unload"));
        assert!(out.contains("max_recording_seconds = 120\n"));
        expect_comments_kept(&example, &out, "#");
    }

    #[test]
    fn replaces_the_top_level_version() {
        let example = example();
        let out = patch_and_check(&example, &[set(path!["version"], json!(2))]);
        assert_eq!(out, rep(&example, "version = 1", "version = 2"));
    }

    #[test]
    fn replaces_a_dotted_key_inside_a_list_item() {
        let example = example();
        let out = patch_and_check(
            &example,
            &[
                set(path!["profiles", 0usize, "insert", "paste_shortcut"], json!("Shift+Insert")),
                set(path!["profiles", 1usize, "cleanup", "llm_enabled"], json!(false)),
            ],
        );
        let expected = rep(
            &rep(&example, "insert.paste_shortcut = \"Ctrl+Shift+V\"", "insert.paste_shortcut = \"Shift+Insert\""),
            "model.active = \"groq/whisper-large-v3-turbo\"\ncleanup.llm_enabled = true",
            "model.active = \"groq/whisper-large-v3-turbo\"\ncleanup.llm_enabled = false",
        );
        assert_eq!(out, expected);
    }

    #[test]
    fn replaces_a_top_level_dotted_key() {
        let source = "# top\ninsert.paste_shortcut = \"Ctrl+V\"   # dotted\nother = 1\n";
        let out = patch_and_check(source, &[set(path!["insert", "paste_shortcut"], json!("Ctrl+Y"))]);
        assert_eq!(out, "# top\ninsert.paste_shortcut = \"Ctrl+Y\"   # dotted\nother = 1\n");
    }

    #[test]
    fn replaces_a_value_inside_an_inline_table() {
        let source = "insert = { method = \"paste\", paste_shortcut = \"Ctrl+V\" }  # inline\n";
        let out = patch_and_check(source, &[set(path!["insert", "method"], json!("type"))]);
        assert_eq!(out, "insert = { method = \"type\", paste_shortcut = \"Ctrl+V\" }  # inline\n");
    }

    #[test]
    fn replaces_a_value_inside_a_nested_inline_table_of_a_list_item() {
        let source = "[[profiles]]\nname = \"A\"\ncleanup = { llm_enabled = true, llm_prompt = \"x\" }\n";
        let out = patch_and_check(source, &[set(path!["profiles", 0usize, "cleanup", "llm_prompt"], json!("casual"))]);
        assert_eq!(out, "[[profiles]]\nname = \"A\"\ncleanup = { llm_enabled = true, llm_prompt = \"casual\" }\n");
    }

    #[test]
    fn replaces_an_array_of_scalars() {
        let example = example();
        let out = patch_and_check(
            &example,
            &[
                set(path!["profiles", 0usize, "match_process"], json!(["wt.exe", "alacritty.exe", "kitty.exe"])),
                set(path!["dictionary", 0usize, "heard"], json!([])),
            ],
        );
        assert!(out.contains("match_process = [\"wt.exe\", \"alacritty.exe\", \"kitty.exe\"]\n"));
        assert!(out.contains("[[dictionary]]\nheard = []\nwrite = \"Tauri\"\n"));
    }

    #[test]
    fn replaces_a_multi_line_array_and_a_literal_string_leaving_neighbours_alone() {
        let source = [
            "[[profiles]]",
            "name = 'Terminals'  # literal",
            "match_process = [",
            "  \"a.exe\",  # first",
            "  \"b.exe\",",
            "]  # after",
            "enabled = true",
            "",
        ]
        .join("\n");
        let out = patch_and_check(
            &source,
            &[
                set(path!["profiles", 0usize, "match_process"], json!(["c.exe"])),
                set(path!["profiles", 0usize, "name"], json!("Shells")),
            ],
        );
        assert_eq!(
            out,
            "[[profiles]]\nname = \"Shells\"  # literal\nmatch_process = [\"c.exe\"]  # after\nenabled = true\n"
        );
    }

    #[test]
    fn replaces_one_element_of_an_array() {
        let out = patch_and_check(&example(), &[set(path!["dictionary", 0usize, "heard", 1usize], json!("tawri"))]);
        assert!(out.contains("heard = [\"tory\", \"tawri\"]"));
    }

    #[test]
    fn handles_keys_that_need_quoting() {
        let source = "[providers.\"my server\"]\nbase_url = \"http://a\"\n\n[\"odd.name\"]\n\"key with space\" = 1\n";
        let out = patch_and_check(
            source,
            &[
                set(path!["providers", "my server", "base_url"], json!("http://b")),
                set(path!["odd.name", "key with space"], json!(2)),
                set(path!["odd.name", "another one"], json!(3)),
            ],
        );
        assert_eq!(
            out,
            "[providers.\"my server\"]\nbase_url = \"http://b\"\n\n[\"odd.name\"]\n\"key with space\" = 2\n\"another one\" = 3\n"
        );
    }

    #[test]
    fn is_not_fooled_by_brackets_hashes_and_equals_signs_inside_strings_and_comments() {
        let source = [
            "# [audio] sounds = true",
            "[prompts.clean]",
            "text = \"use [brackets] # not a comment = here\"  # [audio]",
            "other = 'sounds = \"x\" # y'",
            "",
            "[audio]",
            "sounds = true # sounds = false",
            "",
        ]
        .join("\n");
        let out = patch_and_check(
            &source,
            &[set(path!["audio", "sounds"], json!(false)), set(path!["prompts", "clean", "text"], json!("new"))],
        );
        let expected = rep(
            &rep(&source, "sounds = true # sounds", "sounds = false # sounds"),
            "\"use [brackets] # not a comment = here\"",
            "\"new\"",
        );
        assert_eq!(out, expected);
    }
}

mod identical_values {
    use super::*;

    fn leaf_patches(value: &Value, path: ConfigPath, out: &mut Vec<ConfigPatch>) {
        out.push(set(path.clone(), toml_to_json(value.clone())));
        match value {
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    let mut child = path.clone();
                    child.push(PathSeg::Index(i));
                    leaf_patches(item, child, out);
                }
            }
            Value::Table(table) => {
                for (key, item) in table {
                    let mut child = path.clone();
                    child.push(PathSeg::Key(key.clone()));
                    leaf_patches(item, child, out);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn leaves_the_text_byte_for_byte_unchanged_for_every_path_in_the_example() {
        let example = example();
        let root = parse(&example);
        let mut patches = Vec::new();
        for (key, value) in root.as_table().unwrap() {
            leaf_patches(value, path![key.as_str()], &mut patches);
        }
        assert!(patches.len() > 70);
        for patch in &patches {
            assert_eq!(patch_toml(&example, std::slice::from_ref(patch)).unwrap(), example, "{patch:?}");
        }
        assert_eq!(patch_toml(&example, &patches).unwrap(), example);
    }

    #[test]
    fn leaves_odd_formatting_alone_when_nothing_changes() {
        let source = "a   =   'x'   # c\r\n[t]\r\n  n = 0x10\r\n  f = 1_000.5\r\n  arr = [ 1,\r\n    2 ]\r\n";
        let out = patch_toml(
            source,
            &[
                set(path!["a"], json!("x")),
                set(path!["t", "n"], json!(16)),
                set(path!["t", "f"], json!(1000.5)),
                set(path!["t", "arr"], json!([1, 2])),
                set(path!["t"], json!({ "n": 16, "f": 1000.5, "arr": [1, 2] })),
            ],
        )
        .unwrap();
        assert_eq!(out, source);
    }

    #[test]
    fn treats_removing_a_missing_key_and_emptying_a_missing_list_as_no_ops() {
        let example = example();
        let out = patch_toml(
            &example,
            &[
                remove(path!["general", "nope"]),
                remove(path!["providers", "nobody"]),
                remove(path!["profiles", 7usize]),
                set(path!["local_models"], json!([])),
            ],
        )
        .unwrap();
        assert_eq!(out, example);
    }
}

mod new_keys {
    use super::*;

    #[test]
    fn appends_to_the_end_of_the_table_body_before_the_blank_line() {
        let example = example();
        let out = patch_and_check(&example, &[set(path!["general", "debug_logging"], json!(true))]);
        assert_eq!(
            out,
            rep(
                &example,
                "language = \"en\"               # v1 supports English only\n\n[hotkeys]",
                "language = \"en\"               # v1 supports English only\ndebug_logging = true\n\n[hotkeys]"
            )
        );
    }

    #[test]
    fn appends_to_the_last_table_without_a_trailing_newline() {
        let out = patch_and_check("[audio]\nsounds = true", &[set(path!["audio", "keep_mic_warm"], json!(true))]);
        assert_eq!(out, "[audio]\nsounds = true\nkeep_mic_warm = true\n");
    }

    #[test]
    fn appends_to_an_empty_table() {
        let out =
            patch_and_check("[audio]\n\n[model]\nactive = \"x\"\n", &[set(path!["audio", "sounds"], json!(false))]);
        assert_eq!(out, "[audio]\nsounds = false\n\n[model]\nactive = \"x\"\n");
    }

    #[test]
    fn keeps_comments_that_follow_the_last_key_where_they_are() {
        let source = "[audio]\nsounds = true\n# max_recording_seconds = 300\n\n# about model\n[model]\n";
        let out = patch_and_check(source, &[set(path!["audio", "keep_mic_warm"], json!(true))]);
        assert_eq!(
            out,
            "[audio]\nsounds = true\nkeep_mic_warm = true\n# max_recording_seconds = 300\n\n# about model\n[model]\n"
        );
    }

    #[test]
    fn creates_a_missing_table_at_the_end_of_the_file() {
        let source = "# header\nversion = 1\n\n[audio]\nsounds = true\n";
        let out = patch_and_check(
            source,
            &[set(path!["overlay", "style"], json!("minimal")), set(path!["overlay", "show_waveform"], json!(false))],
        );
        assert_eq!(
            out,
            "# header\nversion = 1\n\n[audio]\nsounds = true\n\n[overlay]\nstyle = \"minimal\"\nshow_waveform = false\n"
        );
    }

    #[test]
    fn creates_a_table_in_an_empty_file_and_in_a_comment_only_file() {
        assert_eq!(patch_and_check("", &[set(path!["audio", "sounds"], json!(false))]), "[audio]\nsounds = false\n");
        assert_eq!(
            patch_and_check("# just a comment", &[set(path!["audio", "sounds"], json!(false))]),
            "# just a comment\n\n[audio]\nsounds = false\n"
        );
    }

    #[test]
    fn puts_a_new_sub_table_next_to_its_siblings() {
        let example = example();
        let out = patch_and_check(
            &example,
            &[
                set(path!["providers", "openai", "api_key"], json!("secret:openai")),
                set(path!["prompts", "email", "text"], json!("Be polite.")),
            ],
        );
        assert!(out.contains(
            "api_key = \"secret:custom-lan\"\n\n[providers.openai]\napi_key = \"secret:openai\"\n\n[cleanup]"
        ));
        assert!(out.contains("tone.\"\n\n[prompts.email]\ntext = \"Be polite.\"\n\n[insert]"));
        expect_comments_kept(&example, &out, "#");
    }

    #[test]
    fn adds_a_top_level_key_before_the_first_table() {
        assert_eq!(
            patch_and_check("[audio]\nsounds = true\n", &[set(path!["version"], json!(1))]),
            "version = 1\n\n[audio]\nsounds = true\n"
        );
        assert_eq!(
            patch_and_check("# Flow settings\n# more\n\n[audio]\nsounds = true\n", &[set(path!["version"], json!(1))]),
            "# Flow settings\n# more\n\nversion = 1\n\n[audio]\nsounds = true\n"
        );
        assert_eq!(
            patch_and_check("a = 1\n\n[audio]\n", &[set(path!["version"], json!(1))]),
            "a = 1\nversion = 1\n\n[audio]\n"
        );
    }

    #[test]
    fn adds_nested_profile_overrides_as_dotted_keys() {
        let out = patch_and_check(
            &example(),
            &[
                set(path!["profiles", 0usize, "cleanup", "filler_removal"], json!(false)),
                set(path!["profiles", 1usize, "insert", "method"], json!("type")),
                set(path!["profiles", 1usize, "enabled"], json!(false)),
            ],
        );
        assert!(
            out.contains(
                "cleanup.llm_enabled = false\ncleanup.filler_removal = false\n\n[[profiles]]\nname = \"Slack\""
            )
        );
        assert!(out.ends_with("cleanup.llm_prompt = \"casual\"\ninsert.method = \"type\"\nenabled = false\n"));
    }

    #[test]
    fn uses_an_existing_sub_table_header_of_a_list_item() {
        let source = [
            "[[profiles]]",
            "name = \"A\"",
            "[profiles.cleanup]",
            "llm_enabled = true  # keep",
            "",
            "[[profiles]]",
            "name = \"B\"",
            "",
        ]
        .join("\n");
        let out = patch_and_check(
            &source,
            &[
                set(path!["profiles", 0usize, "cleanup", "llm_prompt"], json!("email")),
                set(path!["profiles", 0usize, "cleanup", "llm_enabled"], json!(false)),
                set(path!["profiles", 0usize, "insert", "method"], json!("type")),
                set(path!["profiles", 1usize, "cleanup", "llm_prompt"], json!("casual")),
            ],
        );
        assert_eq!(
            out,
            [
                "[[profiles]]",
                "name = \"A\"",
                "insert.method = \"type\"",
                "[profiles.cleanup]",
                "llm_enabled = false  # keep",
                "llm_prompt = \"email\"",
                "",
                "[[profiles]]",
                "name = \"B\"",
                "cleanup.llm_prompt = \"casual\"",
                "",
            ]
            .join("\n")
        );
    }

    #[test]
    fn adds_an_entry_to_an_inline_table() {
        let source = "insert = { method = \"paste\" } # c\nempty = {}\n";
        let out = patch_and_check(
            source,
            &[set(path!["insert", "trailing_space"], json!(false)), set(path!["empty", "a"], json!(1))],
        );
        assert_eq!(out, "insert = { method = \"paste\", trailing_space = false } # c\nempty = { a = 1 }\n");
    }

    #[test]
    fn extends_a_table_spelled_with_dotted_keys_using_dotted_keys() {
        let source = "audio.sounds = true\n\n[model]\nactive = \"x\"\n";
        let out = patch_and_check(source, &[set(path!["audio", "keep_mic_warm"], json!(true))]);
        assert_eq!(out, "audio.sounds = true\naudio.keep_mic_warm = true\n\n[model]\nactive = \"x\"\n");
    }

    #[test]
    fn creates_a_header_when_a_dotted_key_would_clash_with_a_deeper_header() {
        let source = "[a.b.x]\nq = 1\n\n[a]\nk = 1\n";
        let out = patch_and_check(source, &[set(path!["a", "b", "c"], json!(2))]);
        assert_same(
            &parse(&out),
            &to_toml(&json!({ "a": { "k": 1, "b": { "c": 2, "x": { "q": 1 } } } })).unwrap(),
            &out,
        );
        assert!(out.contains("[a.b]\nc = 2\n"));
    }

    #[test]
    fn keeps_a_new_sub_table_header_inside_the_list_item_it_belongs_to() {
        let source = "[[profiles]]\nname = \"A\"\n[profiles.cleanup.deep]\nx = 1\n\n[[profiles]]\nname = \"B\"\n";
        let out = patch_and_check(source, &[set(path!["profiles", 0usize, "cleanup", "llm_enabled"], json!(true))]);
        assert_eq!(
            out,
            "[[profiles]]\nname = \"A\"\n[profiles.cleanup.deep]\nx = 1\n\n[profiles.cleanup]\nllm_enabled = true\n\n[[profiles]]\nname = \"B\"\n"
        );
    }

    #[test]
    fn keeps_crlf_line_endings_on_the_lines_it_adds() {
        let source = example().replace('\n', "\r\n");
        let out = patch_and_check(
            &source,
            &[
                set(path!["general", "debug_logging"], json!(true)),
                set(path!["providers", "openai", "api_key"], json!("secret:openai")),
                set(path!["snippets"], json!([{ "trigger": "a", "text": "one\ntwo" }])),
                remove(path!["audio", "sounds"]),
            ],
        );
        let without = out.replace("\r\n", "");
        assert!(!without.contains(['\r', '\n']), "{out:?}");
        assert!(out.contains("debug_logging = true\r\n"));
        assert!(out.contains("text = \"one\\ntwo\"\r\n"));
        expect_comments_kept(&source, &out, "#");
    }
}

mod removing {
    use super::*;

    #[test]
    fn removes_just_the_key_line() {
        let example = example();
        let out = patch_and_check(&example, &[remove(path!["audio", "sounds"])]);
        assert_eq!(out, rep(&example, "sounds = true\n", ""));
    }

    #[test]
    fn keeps_the_table_header_when_its_last_key_is_removed() {
        let example = example();
        let out = patch_and_check(&example, &[remove(path!["providers", "groq", "api_key"])]);
        assert_eq!(out, rep(&example, "[providers.groq]\napi_key = \"secret:groq\"\n", "[providers.groq]\n"));
    }

    #[test]
    fn treats_null_like_undefined() {
        let example = example();
        let out = patch_toml(&example, &[set(path!["audio", "sounds"], Json::Null)]).unwrap();
        assert_eq!(out, rep(&example, "sounds = true\n", ""));
    }

    #[test]
    fn removes_a_dotted_key_and_a_whole_dotted_table_from_a_list_item() {
        let example = example();
        let patches = [
            remove(path!["profiles", 0usize, "insert", "paste_shortcut"]),
            remove(path!["profiles", 1usize, "cleanup"]),
        ];
        let out = patch_toml(&example, &patches).unwrap();
        // A table spelled only with dotted keys goes when its last key goes.
        assert_same(&prune(&parse(&out)), &prune(&apply_all(&example, &patches)), &out);
        assert!(!out.contains("insert.paste_shortcut"));
        assert!(out.ends_with("match_process = [\"slack.exe\"]\nmodel.active = \"groq/whisper-large-v3-turbo\"\n"));
    }

    #[test]
    fn removes_a_multi_line_value_completely() {
        let source = "[t]\na = [\n  1,\n  2,\n]  # gone\nb = 2\n";
        assert_eq!(patch_and_check(source, &[remove(path!["t", "a"])]), "[t]\nb = 2\n");
    }

    #[test]
    fn removes_entries_of_an_inline_table_and_elements_of_an_array() {
        let source = "t = { a = 1, b = 2, c = 3 }\narr = [\"x\", \"y\", \"z\"]\none = { a = 1 }\n";
        assert!(patch_and_check(source, &[remove(path!["t", "a"])]).contains("t = { b = 2, c = 3 }"));
        assert!(patch_and_check(source, &[remove(path!["t", "c"])]).contains("t = { a = 1, b = 2 }"));
        assert!(patch_and_check(source, &[remove(path!["arr", 1usize])]).contains("arr = [\"x\", \"z\"]"));
        assert!(patch_and_check(source, &[remove(path!["arr", 2usize])]).contains("arr = [\"x\", \"y\"]"));
        assert!(patch_toml(source, &[remove(path!["one", "a"])]).unwrap().contains("one = {}"));
    }

    #[test]
    fn removes_a_whole_table_block_and_the_blank_line_after_it() {
        let example = example();
        let out = patch_and_check(&example, &[remove(path!["providers", "custom-lan"])]);
        assert_eq!(
            out,
            rep(
                &example,
                "[providers.custom-lan]\nbase_url = \"http://192.168.1.20:8000/v1\"\napi_key = \"secret:custom-lan\"\n\n",
                ""
            )
        );
    }

    #[test]
    fn removes_a_parent_table_with_all_its_sub_tables() {
        let example = example();
        let out = patch_toml(&example, &[remove(path!["providers"])]).unwrap();
        assert_eq!(
            out,
            rep(
                &example,
                "[providers.groq]\napi_key = \"secret:groq\"\n\n[providers.custom-lan]\nbase_url = \"http://192.168.1.20:8000/v1\"\napi_key = \"secret:custom-lan\"\n\n",
                ""
            )
        );
    }

    #[test]
    fn leaves_a_comment_that_introduces_the_next_table() {
        let source = "[a]\nx = 1\n\n# about b\n[b]\ny = 2  # gone\n# about c\n[c]\nz = 3\n";
        assert_eq!(patch_and_check(source, &[remove(path!["b"])]), "[a]\nx = 1\n\n# about b\n# about c\n[c]\nz = 3\n");
    }

    #[test]
    fn does_not_leave_blank_lines_at_the_end_of_the_file() {
        let out = patch_and_check(&example(), &[remove(path!["profiles", 1usize])]);
        assert!(out.ends_with("cleanup.llm_enabled = false\n"));
    }
}

mod lists_of_tables {
    use super::*;

    #[test]
    fn replaces_every_block_at_the_position_of_the_first_one() {
        let example = example();
        let dictionary = json!([
            { "heard": ["pie torch"], "write": "PyTorch" },
            { "heard": ["tory", "towery"], "write": "Tauri" },
            { "heard": ["j son"], "write": "JSON" },
        ]);
        let out = patch_and_check(&example, &[set(path!["dictionary"], dictionary)]);
        assert_eq!(
            out,
            rep(
                &example,
                "[[dictionary]]\nheard = [\"tory\", \"towery\"]\nwrite = \"Tauri\"\n",
                &("[[dictionary]]\nheard = [\"pie torch\"]\nwrite = \"PyTorch\"\n\n".to_string()
                    + "[[dictionary]]\nheard = [\"tory\", \"towery\"]\nwrite = \"Tauri\"\n\n"
                    + "[[dictionary]]\nheard = [\"j son\"]\nwrite = \"JSON\"\n"),
            )
        );
    }

    #[test]
    fn serialises_profile_overrides_as_dotted_keys() {
        let example = example();
        let profiles = json!([
            {
                "name": "Editors",
                "match_process": ["code.exe"],
                "match_title": [],
                "enabled": true,
                "cleanup": { "llm_prompt": "code_comment", "filler_removal": false },
                "insert": { "method": "type" },
                "model": {},
            },
            { "name": "Games", "match_title": ["/^Steam/i"], "enabled": false },
        ]);
        let out = patch_and_check(&example, &[set(path!["profiles"], profiles)]);
        let at = out.find("[[profiles]]").unwrap();
        assert_eq!(
            &out[at..],
            [
                "[[profiles]]",
                "name = \"Editors\"",
                "match_process = [\"code.exe\"]",
                "match_title = []",
                "enabled = true",
                "cleanup.llm_prompt = \"code_comment\"",
                "cleanup.filler_removal = false",
                "insert.method = \"type\"",
                "model = {}",
                "",
                "[[profiles]]",
                "name = \"Games\"",
                "match_title = [\"/^Steam/i\"]",
                "enabled = false",
                "",
            ]
            .join("\n")
        );
        assert_eq!(&out[..at], &example[..example.find("[[profiles]]").unwrap()]);
    }

    #[test]
    fn appends_a_list_that_does_not_exist_yet_to_the_end_of_the_file() {
        let example = example();
        let out = patch_and_check(
            &example,
            &[set(path!["local_models"], json!([{ "id": "mine", "path": "C:\\models\\mine.gguf" }]))],
        );
        assert_eq!(out, format!("{example}\n[[local_models]]\nid = \"mine\"\npath = \"C:\\\\models\\\\mine.gguf\"\n"));
    }

    #[test]
    fn removes_the_blocks_when_the_list_is_emptied() {
        let example = example();
        let out =
            patch_toml(&example, &[set(path!["dictionary"], json!([])), set(path!["profiles"], json!([]))]).unwrap();
        assert_eq!(
            out,
            example[..example.find("[[dictionary]]").unwrap()].to_string()
                + "[[snippets]]\ntrigger = \"my address\"\ntext = \"221B Baker Street, London\"\n"
        );
        let parsed = parse(&out);
        assert!(parsed.get("dictionary").is_none());
        assert_eq!(validate_config(&toml_to_json(parsed)).config.dictionary, vec![]);
    }

    #[test]
    fn loses_comments_inside_replaced_blocks_but_keeps_those_around_them() {
        let source = [
            "# top KEEP",
            "[history]",
            "enabled = false  # KEEP",
            "",
            "# before the dictionary KEEP",
            "[[dictionary]]",
            "# inside, lost",
            "heard = [\"a\"]  # lost too",
            "write = \"A\"",
            "",
            "[[dictionary]]",
            "heard = [\"b\"]",
            "write = \"B\"",
            "# after the dictionary KEEP",
            "",
            "[[snippets]]",
            "trigger = \"t\"  # KEEP",
            "text = \"T\"",
            "",
        ]
        .join("\n");
        let out = patch_and_check(&source, &[set(path!["dictionary"], json!([{ "heard": ["c"], "write": "C" }]))]);
        assert_eq!(
            out,
            [
                "# top KEEP",
                "[history]",
                "enabled = false  # KEEP",
                "",
                "# before the dictionary KEEP",
                "[[dictionary]]",
                "heard = [\"c\"]",
                "write = \"C\"",
                "",
                "# after the dictionary KEEP",
                "",
                "[[snippets]]",
                "trigger = \"t\"  # KEEP",
                "text = \"T\"",
                "",
            ]
            .join("\n")
        );
    }

    #[test]
    fn replaces_blocks_together_with_their_sub_table_headers() {
        let source = "[[profiles]]\nname = \"A\"\n[profiles.cleanup]\nllm_enabled = true\n\n[[profiles]]\nname = \"B\"\n\n[audio]\nsounds = true\n";
        let out = patch_and_check(source, &[set(path!["profiles"], json!([{ "name": "C" }]))]);
        assert_eq!(out, "[[profiles]]\nname = \"C\"\n\n[audio]\nsounds = true\n");
    }

    #[test]
    fn rewrites_a_list_written_as_an_inline_array_in_place() {
        let source = "dictionary = [{ heard = [\"a\"], write = \"A\" }]  # inline\n\n[audio]\nsounds = true\n";
        let out = patch_and_check(
            source,
            &[
                set(path!["dictionary"], json!([{ "heard": ["b"], "write": "B" }])),
                set(path!["dictionary", 0usize, "write"], json!("BB")),
                set(path!["dictionary", 1usize], json!({ "heard": ["c"], "write": "C" })),
            ],
        );
        assert_eq!(
            out,
            "dictionary = [{ heard = [\"b\"], write = \"BB\" }, { heard = [\"c\"], write = \"C\" }]  # inline\n\n[audio]\nsounds = true\n"
        );
    }

    #[test]
    fn appends_one_item_after_the_existing_blocks() {
        let example = example();
        let out = patch_and_check(
            &example,
            &[
                set(path!["dictionary", 1usize], json!({ "heard": ["j son"], "write": "JSON" })),
                set(path!["local_models", 0usize], json!({ "id": "m", "path": "m.gguf" })),
            ],
        );
        assert!(
            out.contains("write = \"Tauri\"\n\n[[dictionary]]\nheard = [\"j son\"]\nwrite = \"JSON\"\n\n[[snippets]]")
        );
        assert!(out.ends_with("\n\n[[local_models]]\nid = \"m\"\npath = \"m.gguf\"\n"));
        expect_comments_kept(&example, &out, "#");
    }

    #[test]
    fn removes_one_item_and_keeps_the_others_comments_included() {
        let source = rep(&example(), "name = \"Slack\"", "name = \"Slack\"  # chat");
        let out = patch_and_check(&source, &[remove(path!["profiles", 0usize])]);
        assert_eq!(
            out,
            rep(
                &source,
                "[[profiles]]\nname = \"Terminals\"\nmatch_process = [\"WindowsTerminal.exe\", \"wezterm-gui.exe\"]\ninsert.paste_shortcut = \"Ctrl+Shift+V\"\ncleanup.llm_enabled = false\n\n",
                ""
            )
        );
    }

    #[test]
    fn replaces_one_item_key_by_key_keeping_its_comments() {
        let source = "[[snippets]]\n# the address\ntrigger = \"my address\"  # spoken\ntext = \"old\"\nextra = 1\n";
        let out = patch_and_check(
            source,
            &[set(path!["snippets", 0usize], json!({ "trigger": "my address", "text": "new" }))],
        );
        assert_eq!(out, "[[snippets]]\n# the address\ntrigger = \"my address\"  # spoken\ntext = \"new\"\n");
    }

    #[test]
    fn rejects_an_item_index_past_the_end_of_the_list() {
        let example = example();
        let rejects = |patch: ConfigPatch| matches!(patch_toml(&example, &[patch]), Err(TomlPatchError::Invalid(_)));
        assert!(rejects(set(path!["profiles", 5usize, "name"], json!("x"))));
        assert!(rejects(set(path!["profiles", 3usize], json!({ "name": "x" }))));
        assert!(rejects(set(path!["audio", 0usize], json!(1))));
        assert!(rejects(set(path![], json!(1))));
    }
}

mod whole_tables {
    use super::*;

    #[test]
    fn creates_a_table_from_an_object() {
        let example = example();
        let out = patch_and_check(
            &example,
            &[
                set(
                    path!["providers", "office"],
                    json!({ "base_url": "http://10.0.0.2/v1", "api_key": "secret:office", "model": "whisper-1" }),
                ),
                set(path!["prompts", "mine"], json!({ "text": "Shout." })),
            ],
        );
        assert!(out.contains(
            "\n[providers.office]\nbase_url = \"http://10.0.0.2/v1\"\napi_key = \"secret:office\"\nmodel = \"whisper-1\"\n\n[cleanup]"
        ));
        assert!(out.contains("\n[prompts.mine]\ntext = \"Shout.\"\n\n[insert]"));
        expect_comments_kept(&example, &out, "#");
    }

    #[test]
    fn replaces_the_keys_of_an_existing_table_keeping_comments_on_kept_keys() {
        let source = rep(
            &example(),
            "base_url = \"http://192.168.1.20:8000/v1\"",
            "base_url = \"http://192.168.1.20:8000/v1\"  # the NAS",
        );
        let out = patch_and_check(
            &source,
            &[set(path!["providers", "custom-lan"], json!({ "base_url": "http://nas:9000/v1", "model": "large" }))],
        );
        assert!(out.contains(
            "[providers.custom-lan]\nbase_url = \"http://nas:9000/v1\"  # the NAS\nmodel = \"large\"\n\n[cleanup]"
        ));
    }

    #[test]
    fn creates_an_empty_table() {
        let out = patch_toml("[audio]\nsounds = true\n", &[set(path!["providers", "x"], json!({}))]).unwrap();
        assert_eq!(out, "[audio]\nsounds = true\n\n[providers.x]\n");
        assert_same(
            &parse(&out),
            &to_toml(&json!({ "audio": { "sounds": true }, "providers": { "x": {} } })).unwrap(),
            &out,
        );
    }

    #[test]
    fn empties_a_table_without_removing_it() {
        let out = patch_toml(&example(), &[set(path!["providers", "groq"], json!({}))]).unwrap();
        assert_same(get(&parse(&out), &["providers", "groq"]).unwrap(), &Value::Table(Table::new()), &out);
        assert!(out.contains("[providers.groq]\n\n[providers.custom-lan]"));
    }

    #[test]
    fn sets_a_whole_override_section_of_a_profile_as_dotted_keys() {
        let out = patch_and_check(
            &example(),
            &[
                set(path!["profiles", 1usize, "cleanup"], json!({ "llm_enabled": true, "filler_removal": false })),
                set(path!["profiles", 1usize, "overlay"], json!({ "style": "none" })),
            ],
        );
        assert!(out.ends_with(
            "model.active = \"groq/whisper-large-v3-turbo\"\ncleanup.llm_enabled = true\ncleanup.filler_removal = false\noverlay.style = \"none\"\n"
        ));
    }

    #[test]
    fn turns_a_scalar_into_a_table_and_a_table_into_a_scalar() {
        assert_eq!(
            patch_and_check("[t]\na = 1  # c\n", &[set(path!["t", "a"], json!({ "b": 2 }))]),
            "[t]\na = { b = 2 }  # c\n"
        );
        assert_eq!(
            patch_and_check("[t]\na = 1  # c\n", &[set(path!["t", "a", "b"], json!(2))]),
            "[t]\na = { b = 2 }  # c\n"
        );
        let example = example();
        let out = patch_and_check(&example, &[set(path!["providers", "groq"], json!("nope"))]);
        assert_eq!(get(&parse(&out), &["providers", "groq"]), Some(&Value::String("nope".into())));
        expect_comments_kept(&example, &out, "#");
    }
}

mod strings {
    use super::*;

    const TRICKY: [&str; 13] = [
        "",
        "plain",
        "He said \"hi\"",
        "back\\slash and C:\\Users\\me",
        "tab\there",
        "single 'quotes'",
        "unicode: café – 日本語 – 😀",
        "control: \u{0}\u{7}\u{1f}\u{7f}",
        "# not a comment",
        "ends with quote\"",
        "\"\"\"",
        "a = \"b\" [c] {d}",
        " leading and trailing ",
    ];
    const MULTILINE: [&str; 9] = [
        "two\nlines",
        "trailing newline\n",
        "\nleading newline",
        "windows\r\nline endings\r\n",
        "quotes \"\"\" inside\nand \"\" here\nand at the end\"",
        "backslash at end of line \\\nnext",
        "Dear team,\n\n  Indented \"quote\".\n\tTabbed.\n\nBest,\nA",
        "\"",
        "\n",
    ];

    #[test]
    fn round_trips_tricky_single_line_strings() {
        let example = example();
        for text in TRICKY {
            let out = patch_and_check(
                &example,
                &[
                    set(path!["prompts", "clean", "text"], json!(text)),
                    set(path!["snippets", 0usize, "text"], json!(text)),
                    set(path!["dictionary", 0usize, "heard"], json!([text, "x"])),
                ],
            );
            assert_eq!(lines(&out).len(), lines(&example).len(), "{text:?}");
        }
    }

    #[test]
    fn writes_text_with_line_breaks_as_a_readable_multi_line_string() {
        let out = patch_and_check(
            &example(),
            &[set(path!["prompts", "clean", "text"], json!("Fix grammar.\nKeep the \"tone\".\n"))],
        );
        assert!(out.contains("[prompts.clean]\ntext = \"\"\"\nFix grammar.\nKeep the \"tone\".\n\"\"\"\n\n[insert]"));
    }

    #[test]
    fn round_trips_multi_line_strings_new_and_replaced_in_tables_and_blocks() {
        let example = example();
        for text in MULTILINE {
            patch_and_check(
                &example,
                &[
                    set(path!["prompts", "clean", "text"], json!(text)),
                    set(path!["prompts", "new", "text"], json!(text)),
                    set(path!["snippets"], json!([{ "trigger": "x", "text": text }])),
                    set(path!["snippets", 1usize], json!({ "trigger": "y", "text": text })),
                    set(path!["profiles", 0usize, "cleanup", "note"], json!(text)),
                    set(path!["dictionary", 0usize, "heard"], json!([text])),
                ],
            );
        }
    }

    #[test]
    fn replaces_a_multi_line_string_with_another_value_cleanly() {
        let source = "[prompts.a]\ntext = \"\"\"\nline 1\nline 2 # not a comment\n\"\"\"  # real comment\nx = 1\n";
        assert_eq!(
            patch_and_check(source, &[set(path!["prompts", "a", "text"], json!("short"))]),
            "[prompts.a]\ntext = \"short\"  # real comment\nx = 1\n"
        );
        assert_eq!(
            patch_and_check(source, &[set(path!["prompts", "a", "x"], json!(2))]),
            rep(source, "x = 1", "x = 2")
        );
        assert_eq!(patch_and_check(source, &[remove(path!["prompts", "a", "text"])]), "[prompts.a]\nx = 1\n");
    }

    #[test]
    fn uses_escapes_instead_of_real_line_breaks_in_a_crlf_file_and_inside_inline_values() {
        let crlf = "[prompts.a]\r\ntext = \"x\"\r\n";
        let out = patch_and_check(crlf, &[set(path!["prompts", "a", "text"], json!("one\ntwo"))]);
        assert_eq!(out, "[prompts.a]\r\ntext = \"one\\ntwo\"\r\n");
        let inline = patch_and_check("t = { a = \"x\" }\n", &[set(path!["t", "a"], json!("one\ntwo"))]);
        assert_eq!(inline, "t = { a = \"one\\ntwo\" }\n");
    }

    /// `Infinity`, `-Infinity` and `NaN` from the TS case cannot be expressed:
    /// patch values are JSON, which has no such numbers.
    #[test]
    fn writes_numbers_so_they_read_back_as_the_same_number() {
        let example = example();
        for value in [
            json!(0),
            json!(-1),
            json!(42),
            json!(1.5),
            json!(-0.25),
            json!(1e21),
            json!(1e-7),
            json!(9007199254740992.0),
        ] {
            patch_and_check(&example, &[set(path!["model", "keep_loaded_minutes"], value)]);
        }
    }
}

mod errors {
    use super::*;

    #[test]
    fn throws_on_a_file_that_is_not_valid_toml() {
        match patch_toml("a = = 1\n", &[set(path!["a"], json!(2))]) {
            Err(TomlPatchError::Syntax(error)) => assert!(error.message.contains("Invalid TOML"), "{}", error.message),
            other => panic!("{other:?}"),
        }
    }
}

mod property {
    use super::*;

    /// A small deterministic generator, so a failure can be reproduced from its seed.
    fn mulberry32(seed: u32) -> impl FnMut() -> f64 {
        let mut a = seed as i32;
        move || {
            a = a.wrapping_add(0x6d2b79f5);
            let mut t = (a ^ ((a as u32) >> 15) as i32).wrapping_mul(1 | a);
            t = t.wrapping_add((t ^ ((t as u32) >> 7) as i32).wrapping_mul(61 | t)) ^ t;
            ((t ^ ((t as u32) >> 14) as i32) as u32) as f64 / 4294967296.0
        }
    }

    /// A hand-written file with the layouts the example does not use.
    const MESSY: &str = r#"# Flow settings KEEP
# second line KEEP
version = 1   # KEEP version
insert.method = "paste" # KEEP dotted
insert.trailing_space = true

[general]   # KEEP on header
  theme = 'dark'   # KEEP literal
  launch_at_login = false

# KEEP about audio
[audio]
sounds = true
input_device = """
Microphone (USB)"""
# KEEP commented = "option"

[providers]   # KEEP providers parent
groq = { api_key = "secret:groq" }   # KEEP inline
[providers.local]
base_url = "http://localhost:11434/v1"   # KEEP url

[cleanup]
llm_enabled = true   # KEEP llm

[prompts."my prompt"]   # KEEP quoted
text = "Be brief."

[overlay]
style = "pill"   # KEEP style


# KEEP before lists
[[dictionary]]
heard = [
  "tory",
  "towery",
]
write = "Tauri"

[[profiles]]
name = "Terminals"
match_process = ["wt.exe"]
[profiles.insert]
paste_shortcut = "Ctrl+Shift+V"

# KEEP between profiles
[[profiles]]
name = "Chat"
match_title = ["/slack|discord/i"]
cleanup = { llm_enabled = true }

[history]   # KEEP history
enabled = true
"#;

    const STRINGS: [&str; 11] = [
        "x",
        "",
        "with \"quotes\"",
        "C:\\path\\file",
        "multi\nline\ntext",
        "ends\n",
        "tab\tand # hash",
        "ünïcödé 😀",
        "[not.a.header]",
        "a = 1",
        "\"\"\"",
    ];
    const NEW_KEYS: [&str; 6] = ["extra", "another_one", "key with space", "dotted.name", "ключ", "x-1"];
    const SECTIONS: [&str; 8] = ["general", "hotkeys", "audio", "model", "cleanup", "insert", "overlay", "history"];
    const OVERRIDE_SECTIONS: [&str; 6] = ["audio", "model", "cleanup", "insert", "overlay", "history"];
    const LISTS: [&str; 4] = ["dictionary", "snippets", "profiles", "local_models"];

    struct Generator<R: FnMut() -> f64> {
        random: R,
    }

    fn keys_of(value: Option<&Value>) -> Vec<String> {
        match value {
            Some(Value::Table(table)) => table.keys().cloned().collect(),
            _ => Vec::new(),
        }
    }

    fn is_table(value: Option<&Value>) -> bool {
        matches!(value, Some(Value::Table(_)))
    }

    fn strings(path: &[&str]) -> Vec<String> {
        path.iter().map(|s| s.to_string()).collect()
    }

    fn to_path(path: &[String]) -> ConfigPath {
        path.iter().map(|s| PathSeg::Key(s.clone())).collect()
    }

    impl<R: FnMut() -> f64> Generator<R> {
        fn random(&mut self) -> f64 {
            (self.random)()
        }
        fn int(&mut self, n: usize) -> usize {
            (self.random() * n as f64).floor() as usize
        }
        fn pick<T: Clone>(&mut self, items: &[T]) -> T {
            let i = self.int(items.len());
            items[i].clone()
        }
        fn scalar(&mut self) -> Json {
            match self.int(5) {
                0 | 1 => json!(self.pick(&STRINGS)),
                2 => json!(self.int(1000) as i64 - 500),
                3 => json!((self.int(100000) as f64 - 50000.0) / 128.0),
                _ => json!(self.random() < 0.5),
            }
        }
        fn leaf(&mut self) -> Json {
            if self.random() < 0.8 {
                self.scalar()
            } else {
                let length = self.int(4);
                Json::Array((0..length).map(|_| json!(self.pick(&STRINGS))).collect())
            }
        }
        fn key_of(&mut self, table: Option<&Value>) -> String {
            let existing = keys_of(table);
            if !existing.is_empty() && self.random() < 0.6 {
                self.pick(&existing)
            } else {
                self.pick(&NEW_KEYS).to_string()
            }
        }
        fn item(&mut self, name: &str) -> Json {
            match name {
                "dictionary" => {
                    let heard = self.pick(&STRINGS);
                    json!({ "heard": [heard, "b"], "write": self.pick(&STRINGS) })
                }
                "snippets" => {
                    let trigger = self.pick(&STRINGS);
                    json!({ "trigger": trigger, "text": self.pick(&STRINGS) })
                }
                "local_models" => {
                    let id = self.pick(&STRINGS);
                    json!({ "id": id, "path": self.pick(&STRINGS) })
                }
                _ => {
                    let name = self.pick(&STRINGS);
                    let mut profile = json!({ "name": name, "match_process": [self.pick(&STRINGS)] });
                    if self.random() < 0.5 {
                        profile["enabled"] = json!(self.random() < 0.5);
                    }
                    for section in OVERRIDE_SECTIONS {
                        if self.random() < 0.3 {
                            let key = self.pick(&NEW_KEYS);
                            let value = self.scalar();
                            let mut table = serde_json::Map::new();
                            table.insert(key.to_string(), value);
                            table.insert("llm_prompt".into(), json!(self.pick(&STRINGS)));
                            profile[section] = Json::Object(table);
                        }
                    }
                    profile
                }
            }
        }

        /// Lines with a KEEP comment must not be deleted, so those keys are never removed.
        fn next(&mut self, state: &Value, commented: &HashSet<Vec<String>>) -> ConfigPatch {
            let removable = |path: &[String]| !commented.contains(path);
            loop {
                let roll = self.random();
                if roll < 0.35 {
                    let section = self.pick(&SECTIONS);
                    let key = self.key_of(state.get(section));
                    return set(to_path(&strings(&[section, &key])), self.leaf());
                }
                if roll < 0.45 {
                    let section = self.pick(&SECTIONS);
                    let path = strings(&[section, &self.key_of(state.get(section))]);
                    if removable(&path) {
                        return remove(to_path(&path));
                    }
                } else if roll < 0.55 {
                    let group = self.pick(&["providers", "prompts"]);
                    let name = self.key_of(state.get(group));
                    let table = state.get(group).filter(|g| g.is_table()).and_then(|g| g.get(&name));
                    let key = if is_table(table) || table.is_none() { self.key_of(table) } else { "k".to_string() };
                    let path = strings(&[group, &name, &key]);
                    if table.is_some() && !is_table(table) {
                        continue;
                    }
                    if self.random() < 0.8 {
                        return set(to_path(&path), self.scalar());
                    }
                    if removable(&path) {
                        return remove(to_path(&path));
                    }
                } else if roll < 0.62 {
                    let group = self.pick(&["providers", "prompts"]);
                    let path = strings(&[group, &self.key_of(state.get(group))]);
                    if !removable(&path) {
                        continue;
                    }
                    let mut table = serde_json::Map::new();
                    let mut i = self.int(4);
                    while i > 0 {
                        let key = self.pick(&NEW_KEYS);
                        let value = self.scalar();
                        table.insert(key.to_string(), value);
                        i -= 1;
                    }
                    let blocked = |key: &String| {
                        let mut child = path.clone();
                        child.push(key.clone());
                        !removable(&child)
                    };
                    if table.keys().any(blocked) {
                        continue;
                    }
                    let current = state.get(group).filter(|g| g.is_table()).and_then(|g| g.get(&path[1]));
                    if is_table(current) && keys_of(current).iter().any(blocked) {
                        continue;
                    }
                    return ConfigPatch {
                        path: to_path(&path),
                        value: if self.random() < 0.75 { Some(Json::Object(table)) } else { None },
                    };
                } else if roll < 0.8 {
                    let profiles = match state.get("profiles") {
                        Some(Value::Array(items)) => items.clone(),
                        _ => Vec::new(),
                    };
                    if profiles.is_empty() {
                        continue;
                    }
                    let index = self.int(profiles.len());
                    let profile = &profiles[index];
                    if self.random() < 0.3 {
                        let key = self.pick(&["name", "enabled", "match_process", "match_title"]);
                        let value = match key {
                            "enabled" => json!(self.random() < 0.5),
                            "name" => json!(self.pick(&STRINGS)),
                            _ => json!([self.pick(&STRINGS)]),
                        };
                        return set(path!["profiles", index, key], value);
                    }
                    let section = self.pick(&OVERRIDE_SECTIONS);
                    let current = profile.get(section);
                    if current.is_some() && !is_table(current) {
                        continue;
                    }
                    let key = self.key_of(current);
                    let path = path!["profiles", index, section, key];
                    return ConfigPatch { path, value: if self.random() < 0.8 { Some(self.scalar()) } else { None } };
                } else if roll < 0.88 {
                    let name = self.pick(&LISTS);
                    let length = self.int(4);
                    let items: Vec<Json> = (0..length).map(|_| self.item(name)).collect();
                    return set(path![name], Json::Array(items));
                } else if roll < 0.94 {
                    let name = self.pick(&LISTS);
                    let length = state.get(name).and_then(Value::as_array).map_or(0, Vec::len);
                    return set(path![name, length], self.item(name));
                } else {
                    let name = self.pick(&LISTS);
                    let length = state.get(name).and_then(Value::as_array).map_or(0, Vec::len);
                    if length == 0 {
                        continue;
                    }
                    let index = self.int(length);
                    return if self.random() < 0.5 {
                        remove(path![name, index])
                    } else {
                        set(path![name, index], self.item(name))
                    };
                }
            }
        }
    }

    /// Paths of keys whose line carries a comment that has to survive.
    fn commented_paths(source: &str, marker: &str) -> HashSet<Vec<String>> {
        let header_re = regex::Regex::new(r"^\s*\[([^\[\]]+)\]").unwrap();
        let key_re = regex::Regex::new(r"^\s*([A-Za-z0-9_.-]+)\s*=").unwrap();
        let mut out = HashSet::new();
        let mut table: Vec<String> = Vec::new();
        for line in split_lines(source) {
            let header = header_re.captures(line);
            if let Some(header) = &header {
                table = header[1].split('.').map(|part| part.replace('"', "")).collect();
            }
            if let Some(key) = key_re.captures(line)
                && line.contains(marker)
            {
                let mut path = table.clone();
                path.extend(key[1].split('.').map(str::to_string));
                // The key, and every table above it, must stay.
                for i in 1..=path.len() {
                    out.insert(path[..i].to_vec());
                }
            }
            if header.is_some() && line.contains(marker) {
                for i in 1..=table.len() {
                    out.insert(table[..i].to_vec());
                }
            }
        }
        out
    }

    fn validated(value: &Value) -> Config {
        validate_config(&toml_to_json(prune(value))).config
    }

    fn run(source: &str, seed: u32, steps: usize, marker: &str) {
        let mut next = Generator { random: mulberry32(seed) };
        let commented = commented_paths(source, marker);
        let mut text = source.to_string();
        let mut state = parse(source);
        let mut applied: Vec<ConfigPatch> = Vec::new();
        for step in 0..steps {
            let patch = next.next(&state, &commented);
            applied.push(patch.clone());
            let recent = &applied[applied.len().saturating_sub(3)..];
            let context = format!("seed {seed}, step {step}: {}", serde_json::to_string(recent).unwrap());
            let out = match patch_toml(&text, std::slice::from_ref(&patch)) {
                Ok(out) => out,
                Err(error) => panic!("{context}\n{error}\n--- text ---\n{text}"),
            };
            apply_to_object(&mut state, &patch);
            assert_same(&prune(&parse(&out)), &prune(&state), &format!("{context}\n--- out ---\n{out}"));
            // Applying the same patch again must change nothing (list indexes shift, so skip those).
            if !matches!(patch.path.last(), Some(PathSeg::Index(_))) {
                assert_eq!(patch_toml(&out, std::slice::from_ref(&patch)).unwrap(), out, "{context}");
            }
            let kept = comments(&out, marker);
            for comment in comments(source, marker) {
                assert!(kept.contains(&comment), "{context}\nlost {comment:?}\n--- out ---\n{out}");
            }
            text = out;
        }
        // What the app would load is the same either way.
        assert_eq!(validated(&parse(&text)), validated(&state), "seed {seed}");
    }

    #[test]
    fn mulberry32_matches_the_javascript_generator() {
        // Values printed by the TS implementation for seed 1.
        let mut random = mulberry32(1);
        let first: Vec<f64> = (0..3).map(|_| random()).collect();
        assert_eq!(first, vec![0.6270739405881613, 0.002735721180215478, 0.5274470399599522]);
        let mut random = mulberry32(310);
        let first: Vec<f64> = (0..3).map(|_| random()).collect();
        assert_eq!(first, vec![0.8973127875942737, 0.014486686792224646, 0.6684171659871936]);
    }

    #[test]
    fn re_parses_to_old_parse_plus_patches_and_keeps_every_comment_spec_example() {
        let example = example();
        assert_eq!(comments(&example, "#").len(), 9);
        for seed in 1..=80 {
            run(&example, seed, 40, "#");
        }
    }

    #[test]
    fn holds_for_a_crlf_copy_of_the_example() {
        let crlf = example().replace('\n', "\r\n");
        for seed in 100..130 {
            run(&crlf, seed, 40, "#");
        }
    }

    #[test]
    fn holds_for_a_hand_formatted_file_with_inline_tables_sub_headers_and_odd_spacing() {
        assert_eq!(comments(MESSY, "KEEP").len(), 17);
        for seed in 200..280 {
            run(MESSY, seed, 40, "KEEP");
        }
    }

    #[test]
    fn holds_when_starting_from_an_empty_or_comment_only_file() {
        for seed in 300..310 {
            run("", seed, 40, "#");
        }
        // The comment doubles as a canary: a full rewrite of the file would drop it.
        for seed in 310..340 {
            run("# only a comment KEEP", seed, 40, "KEEP");
        }
    }

    #[test]
    fn applies_a_varied_batch_of_patches_to_the_example_in_one_call() {
        let example = example();
        let patches = vec![
            set(path!["audio", "sounds"], json!(false)),
            set(path!["general", "theme"], json!("light")),
            set(path!["general", "onboarded"], json!(true)),
            set(path!["hotkeys", "hold_to_talk"], json!("RightCtrl")),
            set(path!["model", "active"], json!("groq/whisper-large-v3-turbo")),
            set(path!["model", "keep_loaded_minutes"], json!(0)),
            set(path!["providers", "groq", "api_key"], json!("secret:groq")),
            set(path!["providers", "openai", "api_key"], json!("secret:openai")),
            set(path!["providers", "custom-lan"], json!({ "base_url": "http://nas/v1", "model": "m" })),
            set(path!["prompts", "email", "text"], json!("Rewrite as an email.\nKeep every \"fact\".")),
            remove(path!["prompts", "clean"]),
            set(path!["insert", "match_case"], json!(false)),
            set(path!["overlay", "size"], json!("large")),
            set(path!["history", "enabled"], json!(true)),
            remove(path!["history", "retention_days"]),
            set(path!["dictionary"], json!([{ "heard": ["a", "b"], "write": "C" }, { "heard": ["d"], "write": "E" }])),
            set(path!["snippets", 0usize, "text"], json!("10 Downing Street\nLondon")),
            set(path!["profiles", 1usize, "cleanup", "llm_enabled"], json!(false)),
            set(path!["profiles", 1usize, "overlay", "style"], json!("none")),
            set(path!["profiles", 0usize, "match_title"], json!(["/powershell/i"])),
            set(
                path!["profiles", 2usize],
                json!({ "name": "Games", "match_process": ["*.game.exe"], "enabled": false }),
            ),
            set(path!["local_models"], json!([{ "id": "mine", "path": "D:\\models\\mine.gguf" }])),
        ];
        let out = patch_and_check(&example, &patches);
        expect_comments_kept(&example, &out, "#");
        assert_eq!(validate_config(&toml_to_json(parse(&out))).issues, vec![]);
        assert_eq!(patch_toml(&out, &patches).unwrap(), out);
    }
}
