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
from datetime import date, datetime, timedelta
from decimal import Decimal
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
    types = {'observed_at': datetime, 'observed_on': date, 'accepted': bool, 'attempts': int, 'amount': Decimal, 'note': str, 'choice': str}
    for field, expected_type in types.items():
        assert getattr(p, f'decide_clear_observation_{field}')() is None
        expression = getattr(s, f'decide_clear_observation_{field}')()
        assert expression.type.python_type is expected_type, (field, expression.type)
        assert db.scalar(select(expression)) is None
"#,
    );
}

#[test]
fn policy_headers_preserve_separate_input_paths_with_spaces() {
    let dir = tempfile::tempdir().unwrap();
    let inputs = [
        "a long first directory with spaces/source_one_for_python_provenance.rdra",
        "a long second directory with spaces/source_two_for_python_provenance.rdra",
    ];
    for (index, path) in inputs.iter().enumerate() {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            format!("dto Record{index} \"Record {index}\" {{ value: Int }}"),
        )
        .unwrap();
    }
    for (lang, file) in [
        ("py-policy", "policy.py"),
        ("py-sql-policy", "sql_policy.py"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_rdra-ish"))
            .current_dir(dir.path())
            .args(["codegen", inputs[0], inputs[1], "--lang", lang, "-o", file])
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
import ast
from pathlib import Path
paths = (
    'a long first directory with spaces/source_one_for_python_provenance.rdra',
    'a long second directory with spaces/source_two_for_python_provenance.rdra',
)
for name in ('policy.py', 'sql_policy.py'):
    content = Path(name).read_text()
    doc = ast.get_docstring(ast.parse(content))
    assert doc is not None
    assert f'Source: {paths[0]}\n{paths[1]}.' in doc
    assert all(len(line) <= 100 for line in content.splitlines())
"#,
    );
}

#[test]
fn shared_guards_preserve_three_guarded_destinations_and_rejection() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("model.rdra"),
        r#"
variation Choice "Choice" { accept reject skip }
entity Job "Job" { id: Int @pk status: Enum(open, accepted, rejected, skipped) @default(open) }
entity Context "Invocation" { enabled: Bool healthy: Bool choice: Choice }
relate(Context, Job, N:1)
condition Enabled "Enabled"
holds(Enabled, Context.enabled == true)
condition Healthy "Healthy"
holds(Healthy, Context.healthy == true)
condition Accept "Accept"
holds(Accept, Context.choice == accept)
condition Reject "Reject"
holds(Reject, Context.choice == reject)
condition Skip "Skip"
holds(Skip, Context.choice == skip)
usecase Finish "Finish"
creates(Finish, Context)
updates(Finish, Job)
chooses(Finish, Context.enabled)
chooses(Finish, Context.healthy)
chooses(Finish, Context.choice)
applies_to(Enabled, Finish)
applies_to(Healthy, Finish)
transitions(Finish, Job.status, open -> accepted).when(Accept)
transitions(Finish, Job.status, open -> rejected).when(Reject)
transitions(Finish, Job.status, open -> skipped).when(Skip)
"#,
    )
    .unwrap();
    for (language, file) in [
        ("py-policy", "policy.py"),
        ("py-sql-policy", "sql_policy.py"),
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_rdra-ish"))
            .current_dir(dir.path())
            .args(["codegen", "model.rdra", "--lang", language, "-o", file])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    python(
        &dir,
        r#"
from itertools import product
from unittest import TestCase
from sqlalchemy import create_engine, literal, select
import policy as p
import sql_policy as s
outcomes = {'accept': 'accepted', 'reject': 'rejected', 'skip': 'skipped'}
with create_engine('sqlite://').connect() as db:
    for enabled, healthy, choice, status in product((False, True), (False, True), outcomes, ('open', 'accepted', 'rejected', 'skipped')):
        inputs = dict(context_enabled=enabled, context_healthy=healthy, context_choice=choice, job_status=status)
        expected = outcomes[choice] if enabled and healthy and status == 'open' else None
        if expected is None:
            with TestCase().assertRaises(ValueError):
                p.decide_finish_status(**inputs)
        else:
            assert p.decide_finish_status(**inputs) == expected
        actual = db.scalar(select(s.decide_finish_status(**{key: literal(value) for key, value in inputs.items()})))
        assert actual == expected, (inputs, expected, actual)
"#,
    );
}

#[test]
fn clock_effects_use_the_supplied_clock_in_python_and_sql() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("model.rdra"),
        r#"
entity Stamp "Stamp" { id: Int @pk created_at: DateTime touched_at: DateTime @null enabled: Bool }
usecase Open "Open"
usecase Touch "Touch"
creates(Open, Stamp)
sets(Open, Stamp.created_at == now)
sets(Open, Stamp.touched_at == null)
updates(Touch, Stamp)
condition Enabled "Enabled"
holds(Enabled, Stamp.enabled == true)
applies_to(Enabled, Touch)
sets(Touch, Stamp.touched_at == now)
"#,
    )
    .unwrap();
    for (language, file) in [
        ("py-policy", "policy.py"),
        ("py-sql-policy", "sql_policy.py"),
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_rdra-ish"))
            .current_dir(dir.path())
            .args(["codegen", "model.rdra", "--lang", language, "-o", file])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    python(
        &dir,
        r#"
from datetime import datetime, timedelta
from unittest import TestCase
from sqlalchemy import create_engine, literal, select
from sqlalchemy.pool import NullPool
import policy as p
import sql_policy as s
with create_engine('sqlite://', poolclass=NullPool).connect() as db:
    for offset in range(6):
        now = datetime(2026, 1, 1) + timedelta(seconds=offset)
        assert p.decide_open_created_at(now=now) == now
        assert db.scalar(select(s.decide_open_created_at(now=literal(now)))) == now
        for enabled in (False, True):
            if enabled:
                assert p.decide_touch_touched_at(stamp_enabled=enabled, now=now) == now
            else:
                with TestCase().assertRaises(ValueError):
                    p.decide_touch_touched_at(stamp_enabled=enabled, now=now)
            actual = db.scalar(select(s.decide_touch_touched_at(stamp_enabled=literal(enabled), now=literal(now))))
            assert actual == (now if enabled else None)
"#,
    );
}

#[test]
fn clock_effects_satisfy_tlc_after_assertions_at_both_instance_bounds() {
    let available = Command::new("which")
        .arg("tlc")
        .output()
        .unwrap()
        .status
        .success();
    if !available {
        assert_ne!(std::env::var("RDRA_REQUIRE_TLC").as_deref(), Ok("1"));
        println!("skipped clock effects: tlc is not on PATH");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = r#"
entity Stamp "Stamp" { id: Int @pk created_at: DateTime touched_at: DateTime @null }
usecase Open "Open"
usecase Touch "Touch"
creates(Open, Stamp)
sets(Open, Stamp.created_at == now)
sets(Open, Stamp.touched_at == null)
updates(Touch, Stamp)
sets(Touch, Stamp.touched_at == now)
after(Open).assert(Stamp.created_at == now, Stamp.touched_at == null)
after(Touch).assert(Stamp.touched_at == now)
"#;
    let model = dir.path().join("model.rdra");
    for count in ["1", "2"] {
        for precise in [true, false] {
            let source = if precise {
                source.to_string()
            } else {
                source.replace(
                    "sets(Touch, Stamp.touched_at == now)",
                    "sets(Touch, Stamp.touched_at == present)",
                )
            };
            std::fs::write(&model, source).unwrap();
            let result = Command::new(env!("CARGO_BIN_EXE_rdra-ish"))
                .current_dir(dir.path())
                .env("TMPDIR", dir.path())
                .args([
                    "verify",
                    "model.rdra",
                    "--instance-count",
                    count,
                    "--deny-undecided",
                ])
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&result.stdout);
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert_eq!(result.status.success(), precise, "{stdout}{stderr}");
            if precise {
                assert!(stdout.contains("tlc_result: ok"), "{stdout}{stderr}");
            } else {
                assert_eq!(result.status.code(), Some(1), "{stdout}{stderr}");
                assert!(
                    stderr.contains("error[AfterAssert_Touch_1]"),
                    "{stdout}{stderr}"
                );
                assert!(
                    stderr.contains("tlc_result: counterexample"),
                    "{stdout}{stderr}"
                );
            }
        }
    }
}

#[test]
fn birth_history_quotient_preserves_tlc_failures_and_bounded_progress() {
    if !Command::new("which")
        .arg("tlc")
        .output()
        .unwrap()
        .status
        .success()
    {
        assert_ne!(std::env::var("RDRA_REQUIRE_TLC").as_deref(), Ok("1"));
        println!("skipped birth-history verification: tlc is not on PATH");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = r#"
entity Task "Task" { id: Int @pk status: Enum(open, done) @default(open) }
entity Input "Input" { cutoff: DateTime observed: DateTime enabled: Bool }
relate(Input, Task, N:1)
usecase Open "Open"
creates(Open, Task)
sets(Open, Task.status == open)
usecase Finish "Finish"
updates(Finish, Task)
creates(Finish, Input)
chooses(Finish, Input.cutoff)
chooses(Finish, Input.observed)
chooses(Finish, Input.enabled)
condition Accept "Accept"
holds(Accept, Input.enabled == true, Input.observed < Input.cutoff).along(Input, Task)
applies_to(Accept, Finish)
transitions(Finish, Task.status, open -> done)
"#;
    for count in ["1", "2"] {
        for failing in [false, true] {
            let model = if failing {
                format!("{source}\nforbidden(Task, status == done)")
            } else {
                source.to_string()
            };
            std::fs::write(dir.path().join("model.rdra"), model).unwrap();
            let result = Command::new(env!("CARGO_BIN_EXE_rdra-ish"))
                .current_dir(dir.path())
                .env("TMPDIR", dir.path())
                .args([
                    "verify",
                    "model.rdra",
                    "--instance-count",
                    count,
                    "--deny-undecided",
                ])
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&result.stdout);
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert_eq!(result.status.success(), !failing, "{stdout}{stderr}");
            assert!(
                stdout.contains("birth-only history quotient:"),
                "{stdout}{stderr}"
            );
            assert!(!stdout.contains("not checked:"), "{stdout}{stderr}");
            if failing {
                assert!(
                    stderr.contains("tlc_result: counterexample"),
                    "{stdout}{stderr}"
                );
                assert!(
                    stderr.contains("error[Forbidden_Task_0]"),
                    "{stdout}{stderr}"
                );
            } else {
                assert!(stdout.contains("tlc_result: ok"), "{stdout}{stderr}");
            }
        }
    }
}
