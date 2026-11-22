//! Existing Python domains need their nested shapes and constructor contracts preserved.

use std::process::Command;

const MODEL: &str = r#"
variation Model "Model" { "model-3.1" "model-3.8" }
dto Envelope "Envelope" {
    user: User
    items: List(User) @default(empty)
    tags: Tuple(String) @default(empty)
    selected: Model @null @default(null)
    active: Bool @default(true)
    count: Int @default(01)
}
dto User "User"
    description "Attributes:
    name: The user name.
    note: An optional note." {
    name: String
    note: String @null @default(null)
}
"#;

fn generated(config: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("model.rdra"), MODEL).unwrap();
    std::fs::write(dir.path().join("python.json"), config).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_rdra-ish"))
        .current_dir(dir.path())
        .args([
            "codegen",
            "model.rdra",
            "--lang",
            "py",
            "--python-config",
            "python.json",
            "-o",
            "domain.py",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    dir
}

fn python(dir: &tempfile::TempDir, program: &str) {
    let interpreter = std::env::var("RDRA_TEST_PYTHON").unwrap_or_else(|_| "python3".into());
    let result = Command::new(interpreter)
        .current_dir(dir.path())
        .args(["-c", program])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn nested_dataclasses_preserve_optional_defaults_lists_and_mutability() {
    let dir = generated(r#"{"mutable_records":["User"],"slots":true,"literal_value_sets":true}"#);
    python(
        &dir,
        r#"
from dataclasses import FrozenInstanceError
from typing import get_args
import domain as d
user = d.User(name="before")
assert user.note is None
assert "    name: The user name." in d.User.__doc__
assert "    note: An optional note." in d.User.__doc__
user.name = "after"
a = d.Envelope(user=user)
b = d.Envelope(user=user)
assert a.active is True and a.tags == () and a.selected is None and a.count == 1
a.items.append(user)
assert not b.items, "list defaults must not share storage"
assert not hasattr(user, "__dict__"), "slots must be retained"
assert set(get_args(d.Model)) == {"model-3.1", "model-3.8"}
try:
    a.active = False
except FrozenInstanceError:
    pass
else:
    raise AssertionError("an immutable record must reject mutation")
"#,
    );
}

#[test]
fn pydantic_records_preserve_validation_and_json_serialization() {
    let dir = generated(r#"{"pydantic_records":["User","Envelope"],"literal_value_sets":true}"#);
    python(
        &dir,
        r#"
import domain as d
from pydantic import ValidationError
value = d.Envelope.model_validate({"user":{"name":"test"}, "selected":"model-3.8"})
assert value.model_dump(mode="json")["user"] == {"name":"test", "note":None}
assert value.selected == "model-3.8"
for payload in ({"user":{"name":"test"}, "extra":1}, {"user":{"name":"test"}, "selected":"unknown"}):
    try:
        d.Envelope.model_validate(payload)
    except ValidationError:
        pass
    else:
        raise AssertionError("the generated contract must reject invalid payloads")
"#,
    );
}

#[test]
fn quoted_values_generate_valid_enum_identifiers_and_keep_wire_strings() {
    let dir = generated("{}");
    python(
        &dir,
        r#"
import domain as d
assert {v.value for v in d.Model} == {"model-3.1", "model-3.8"}
assert d.Envelope(user=d.User(name="test")).selected is None
"#,
    );
}

#[test]
fn nested_schema_references_resolve_to_published_definitions() {
    let dir = generated("{}");
    for (kind, pointer, target) in [
        (
            "json-schema",
            "/$defs/Dto.Envelope/properties/user/$ref",
            "#/$defs/Dto.User",
        ),
        (
            "openapi",
            "/components/schemas/Envelope/properties/user/$ref",
            "#/components/schemas/User",
        ),
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_rdra-ish"))
            .current_dir(dir.path())
            .args(["export", "model.rdra", "--kind", kind, "-o", "schema.json"])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let schema: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("schema.json")).unwrap())
                .unwrap();
        assert_eq!(
            schema.pointer(pointer).and_then(|v| v.as_str()),
            Some(target)
        );
        assert!(schema.pointer(target.strip_prefix('#').unwrap()).is_some());
    }
}

#[test]
fn representation_options_reject_typographical_errors_and_unknown_records() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("model.rdra"), MODEL).unwrap();
    for config in [
        r#"{"slot":true}"#,
        r#"{"mutable_records":["Missing"]}"#,
        r#"{"enum_value_sets":["Missing"]}"#,
    ] {
        std::fs::write(dir.path().join("python.json"), config).unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_rdra-ish"))
            .current_dir(dir.path())
            .args([
                "codegen",
                "model.rdra",
                "--lang",
                "py",
                "--python-config",
                "python.json",
            ])
            .output()
            .unwrap();
        assert!(!result.status.success());
    }
}
