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

#[test]
fn numeric_conditions_compare_columns_and_preserve_nullable_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let sample = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../samples/python-domain/numeric_policy.rdra");
    let output = Command::new(env!("CARGO_BIN_EXE_rdra-ish"))
        .args(["codegen"])
        .arg(sample)
        .args(["--lang", "py", "-o"])
        .arg(dir.path().join("domain.py"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    python(
        &dir,
        r#"
import domain as d
assert not hasattr(d, 'Unsupported')
for left in (None, -1, 0, 1, 2, 6):
    for right in (None, -1, 0, 1, 2, 6):
        row = d.NumbersReady(id=d.NumbersId(1), left=left, right=right)
        present = left is not None and right is not None
        assert isinstance(d.less(row), d.Rejected) == (not (present and left < right))
        assert isinstance(d.equal(row), d.Rejected) == (not (left == right))
        assert isinstance(d.literal(row), d.Rejected) == (not (left is not None and left < 2))
        assert isinstance(d.missing(row), d.Rejected) == (left is not None)
assert d.observe(d.NumbersId(2), 7, 12).numbers.left == 7
"#,
    );
}

#[test]
fn keyword_variation_defaults_remain_bare_values() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("model.rdra"),
        r#"variation Access "Access" { permission actor }
dto Settings "Settings" { access: Access @default(permission) }"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rdra-ish"))
        .current_dir(dir.path())
        .args(["codegen", "model.rdra", "--lang", "py", "-o", "domain.py"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    python(
        &dir,
        "import domain as d; assert d.Settings().access.value == 'permission'",
    );
}

#[test]
fn policy_predicates_and_state_decisions_execute_as_python_and_sql() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../samples/python-domain/numeric_policy.rdra"
        ),
        dir.path().join("model.rdra"),
    )
    .unwrap();
    for (language, file) in [
        ("py-policy", "policy.py"),
        ("py-sql-policy", "sql_policy.py"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_rdra-ish"))
            .current_dir(dir.path())
            .args(["codegen", "model.rdra", "--lang", language, "-o", file])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    python(
        &dir,
        r#"
from itertools import product
from sqlalchemy import create_engine, literal, select
import policy as p
import sql_policy as s
engine = create_engine('sqlite://')
with engine.connect() as connection:
    for left, right in product((None, -1, 0, 1, 2, 6), repeat=2):
        inputs = {'numbers_left': left, 'numbers_right': right}
        sql_inputs = {key: literal(value) for key, value in inputs.items()}
        expected = left is not None and right is not None and left < right
        assert p.equal_guard(**inputs) == (left == right)
        assert p.not_equal_guard(**inputs) == (left != right)
        assert p.different_guard(numbers_left=left) == (left != 2)
        assert bool(connection.scalar(select(s.equal_guard(**sql_inputs)))) == (left == right)
        assert bool(connection.scalar(select(s.not_equal_guard(**sql_inputs)))) == (left != right)
        assert bool(connection.scalar(select(s.different_guard(numbers_left=literal(left))))) == (left != 2)
        inverse_inputs = {'optional_facts_value': left, 'optional_facts_other': right}
        assert p.inverse_guard(**inverse_inputs) == (not expected)
        assert bool(connection.scalar(select(s.inverse_guard(**{key: literal(value) for key, value in inverse_inputs.items()})))) == (not expected)
        assert p.less_guard(**inputs) == expected
        assert bool(connection.scalar(select(s.less_guard(**sql_inputs)))) == expected
        if expected:
            assert p.decide_less_status(numbers_status='ready', **inputs) == 'accepted'
            sql_values = {'numbers_status': literal('ready'), **sql_inputs}
            assert connection.scalar(select(s.decide_less_status(**sql_values))) == 'accepted'
        else:
            try:
                p.decide_less_status(numbers_status='ready', **inputs)
            except ValueError:
                pass
            else:
                raise AssertionError('A disabled transition must reject execution')
    for flag in (None, False, True):
        assert p.false_flag(optional_facts_flag=flag) == (flag is False)
        assert bool(connection.scalar(select(s.false_flag(optional_facts_flag=literal(flag))))) == (flag is False)
    assert p.missing_guard(numbers_left=None)
    assert not p.missing_guard(numbers_left=0)
"#,
    );
}

#[test]
fn disjoint_guarded_destinations_execute_without_branch_priority() {
    let dir = tempfile::tempdir().unwrap();
    let model = r#"
entity Task "Task" {
    status: Enum(open, accepted, rejected) @default(open)
}

entity FinishInput "Invocation input" {
    approved: Bool
    accepted: Bool @derived(approved == true)
}
condition Acceptable "Accept"
holds(Acceptable, FinishInput.accepted == true)
condition Rejectable "Reject"
holds(Rejectable, FinishInput.accepted == false)
usecase Finish "Finish task"
creates(Finish, FinishInput)
chooses(Finish, FinishInput.approved)
updates(Finish, Task)
transitions(Finish, Task.status, open -> accepted).when(Acceptable)
transitions(Finish, Task.status, open -> rejected).when(Rejectable)
"#;
    std::fs::write(dir.path().join("model.rdra"), model).unwrap();
    for (lang, file) in [
        ("py-policy", "policy.py"),
        ("py-sql-policy", "sql_policy.py"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_rdra-ish"))
            .current_dir(dir.path())
            .args(["codegen", "model.rdra", "--lang", lang, "-o", file])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    python(
        &dir,
        r#"
import policy as p
import sql_policy as s
from sqlalchemy import create_engine, literal, select
with create_engine('sqlite://').connect() as db:
    for approved, expected in ((True, 'accepted'), (False, 'rejected')):
        assert p.decide_finish_status(task_status='open', finish_input_approved=approved) == expected
        actual = db.scalar(select(s.decide_finish_status(task_status=literal('open'), finish_input_approved=literal(approved))))
        assert actual == expected, (approved, actual)
"#,
    );
    std::fs::write(
        dir.path().join("model.rdra"),
        model.replace("FinishInput.accepted == false", "Task.status == open"),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rdra-ish"))
        .current_dir(dir.path())
        .args(["codegen", "model.rdra", "--lang", "py-policy"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("provably disjoint guards"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::write(
        dir.path().join("model.rdra"),
        model.replace("approved: Bool", "approved: Bool @null"),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rdra-ish"))
        .current_dir(dir.path())
        .args(["codegen", "model.rdra", "--lang", "py-policy"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("nullable Bool input choices are not supported"));
}

#[test]
fn column_effects_preserve_nullable_deadlines_and_supplied_timestamps() {
    let dir = tempfile::tempdir().unwrap();
    let model = include_str!("../../../samples/python-domain/column_effects.rdra");
    std::fs::write(dir.path().join("model.rdra"), model).unwrap();
    for (lang, file) in [
        ("py-policy", "policy.py"),
        ("py-sql-policy", "sql_policy.py"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_rdra-ish"))
            .current_dir(dir.path())
            .args(["codegen", "model.rdra", "--lang", lang, "-o", file])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    python(
        &dir,
        r#"
from datetime import datetime, timedelta
from itertools import product
from typing import get_args
from sqlalchemy import create_engine, literal, select
import policy as p
import sql_policy as s
now = datetime(2026, 1, 1)
assert set(get_args(p.SNAPSHOT_STATUSValue)) == {'available', 'deleted', 'expired'}
assert set(get_args(p.METADATA_SOURCEValue)) == {'manual', 'background'}
assert not hasattr(p, 'OBSERVATION_SOURCE'), 'Typed inputs must not duplicate unlabeled metadata sets'
with create_engine('sqlite://').connect() as db:
    for deleted, expiry in product((None, now), (None, now - timedelta(seconds=1), now, now + timedelta(seconds=1))):
        expected = 'deleted' if deleted is not None or expiry is None else 'expired' if expiry < now else 'available'
        assert p.decide_observe_status(metadata_deleted_at=deleted, metadata_expires_at=expiry, now=now) == expected
        actual = db.scalar(select(s.decide_observe_status(metadata_deleted_at=literal(deleted), metadata_expires_at=literal(expiry), now=literal(now))))
        assert actual == expected, (deleted, expiry, actual)
    assert p.decide_observe_observed_at(new_snapshot_observed_at=now) is now
    assert db.scalar(select(s.decide_observe_observed_at(new_snapshot_observed_at=literal(now)))) == now
    assert p.decide_clear_observation_observed_at() is None
    assert db.scalar(select(s.decide_clear_observation_observed_at())) is None
"#,
    );
}
