//! Compare every class member's labelled future with the unreduced graph, not just totals.

use std::collections::{BTreeMap, BTreeSet};

use super::BirthHistory;
use crate::lifecycle::explore::ComponentRun;
use crate::lifecycle::findings::{Progress, Runs, Taken, Witness};
use crate::lifecycle::tests::model_from;
use crate::lifecycle::types::ActId;
use crate::lifecycle::{Bounds, Caps, Scope, State, TransitionSystem};

const MODEL: &str = r#"
entity Task "Task" { id: Int @pk status: Enum(open, done) @default(open) @end(done) }
entity Invocation "Invocation" { cutoff: DateTime observed: DateTime go: Bool }
relate(Invocation, Task, N:1)
usecase Open "Open"
creates(Open, Task)
sets(Open, Task.status == open)
usecase Evaluate "Evaluate"
creates(Evaluate, Invocation)
updates(Evaluate, Task)
chooses(Evaluate, Invocation.cutoff)
chooses(Evaluate, Invocation.observed)
chooses(Evaluate, Invocation.go)
condition Accept "Accept"
holds(Accept, Invocation.go == true, Invocation.observed < Invocation.cutoff).along(Invocation, Task)
condition Decline "Decline"
holds(Decline, Invocation.go == false).along(Invocation, Task)
condition Late "Late"
holds(Late, Invocation.observed >= Invocation.cutoff).along(Invocation, Task)
transitions(Evaluate, Task.status, open -> done).when(Accept)
transitions(Evaluate, Task.status, open -> open).when(Decline)
transitions(Evaluate, Task.status, open -> open).when(Late)
"#;

type Future = BTreeSet<(ActId, Box<[u8]>, State)>;

fn futures(run: &ComponentRun, history: &BirthHistory) -> Vec<Future> {
    let mut out = vec![BTreeSet::new(); run.states.len()];
    for edge in run.edges() {
        let (act, params) = run.step(edge);
        out[edge.from as usize].insert((
            act,
            params.into(),
            history.key(&run.states[edge.to as usize]).into_owned(),
        ));
    }
    out
}

fn findings(run: &ComponentRun) -> Vec<String> {
    let ts = &run.system;
    let mut out = Vec::new();
    for entity in ts.entities() {
        let e = ts
            .entities()
            .iter()
            .position(|x| x.id == entity.id)
            .unwrap();
        let f = ts.state_model_findings(crate::lifecycle::EntId::at(e), &run.graph());
        for w in f.witnesses {
            let result = match w.witness {
                Witness::At(_) => "reached".to_string(),
                Witness::Missing(d) => d.name(&[]),
                Witness::NotDecided => "undecided".to_string(),
            };
            out.push(format!("{} {:?} {:?} {result}", entity.id, w.var, w.value));
        }
        for d in f.deadlocks {
            let result = match d.progress {
                Progress::Leaves => "leaves".to_string(),
                Progress::Stuck { diag, .. } => format!("{} {:?}", diag.name(&[]), diag.severity()),
                Progress::NotDecided => "undecided".to_string(),
            };
            out.push(format!("{} {:?} {:?} {result}", entity.id, d.var, d.value));
        }
        for a in f.runs {
            let result = match a.runs {
                Runs::Fired => "fired".to_string(),
                Runs::Never(d) => d.name(&[]),
                Runs::NotDecided => "undecided".to_string(),
            };
            out.push(format!("{:?} {result}", a.act));
        }
        for arm in f.arms {
            let result = match arm.taken {
                Taken::Yes => "taken".to_string(),
                Taken::Never(d) => d.name(&[]),
                Taken::Moot => "moot".to_string(),
                Taken::NotDecided => "undecided".to_string(),
            };
            out.push(format!("{:?} {result}", arm.site.fact));
        }
    }
    out.sort();
    out
}

#[test]
fn birth_history_preserves_every_labelled_future_finding_and_lifted_trace() {
    for bounds in [Bounds::STATES, Bounds::VERIFY] {
        let ts = TransitionSystem::build(&model_from(MODEL), bounds, &Scope::ALL).ts;
        let history = BirthHistory::of(&ts);
        assert_eq!(history.len(), 3);
        let caps = Caps::new(256, 200_000);
        let raw = ts.clone().run(caps);
        let quotient = ts.clone().run_state_models(caps);
        assert!(raw.complete && quotient.complete);
        assert!(quotient.states.len() * 5 < raw.states.len());
        let mut expected = BTreeMap::new();
        for (state, future) in raw.states.iter().zip(futures(&raw, &history)) {
            let key = history.key(state).into_owned();
            if let Some(previous) = expected.insert(key, future.clone()) {
                assert_eq!(
                    previous, future,
                    "every member of a class has the same labelled future"
                );
            }
        }
        let actual: BTreeMap<_, _> = quotient
            .states
            .iter()
            .zip(futures(&quotient, &history))
            .map(|(s, future)| (history.key(s).into_owned(), future))
            .collect();
        assert_eq!(expected, actual);
        assert_eq!(findings(&raw), findings(&quotient));
        for at in 0..quotient.states.len() {
            let (start, steps) = quotient.lifted_path(at as u32, None);
            let actions: Vec<_> = steps.iter().map(|s| s.0).collect();
            let states = std::iter::once(start)
                .chain(steps.into_iter().map(|s| s.2))
                .collect();
            assert!(ts.replay(states, &actions).is_ok());
        }
        // Bounds must still exhaust the two existing invocation slots after two calls.
        let exhausted = quotient.states.iter().any(|s| {
            let input = ts
                .entities()
                .iter()
                .position(|e| e.id == "Invocation")
                .unwrap();
            let e = crate::lifecycle::EntId::at(input);
            (0..bounds.slots()).all(|slot| s.is_present(&ts, e, slot))
                && ts
                    .moves(s)
                    .all(|m| ts.actions()[m.act.index()].name != "Evaluate")
        });
        assert!(exhausted);
    }
}

#[test]
fn birth_history_retains_checks_later_reads_mutations_and_guarded_choices() {
    for (extra, expected) in [
        ("forbidden(Invocation, observed < cutoff)", 1),
        ("after(Evaluate).assert(Invocation.go == true)", 2),
        ("property Chosen always(Invocation.go == true)", 2),
        ("usecase Replace \"Replace\"\nupdates(Replace, Invocation)\nchooses(Replace, Invocation.go)", 0),
        ("usecase Audit \"Audit\"\nupdates(Audit, Task)\ncondition Old \"Old\"\nholds(Old, Invocation.observed < Invocation.cutoff)\napplies_to(Old, Audit)\ntransitions(Audit, Task.status, open -> done)", 1),
    ] {
        let ts = TransitionSystem::build(&model_from(&format!("{MODEL}\n{extra}")), Bounds::STATES, &Scope::ALL).ts;
        assert_eq!(BirthHistory::of(&ts).len(), expected, "{extra}");
    }
    let guarded = "variation Route \"Route\" { a b }\n".to_string() + &MODEL.replace(
        "go: Bool }", "go: Bool choice: Route }",
    ) + "\nchooses(Evaluate, Invocation.choice, a).when(Accept)\nchooses(Evaluate, Invocation.choice, b).when(Decline)";
    let ts = TransitionSystem::build(&model_from(&guarded), Bounds::STATES, &Scope::ALL).ts;
    assert_eq!(BirthHistory::of(&ts).len(), 3);
    let raw = ts.clone().run(Caps::VERIFY);
    let quotient = ts.run_state_models(Caps::VERIFY);
    assert!(raw.complete && quotient.complete);
    assert_eq!(findings(&raw), findings(&quotient));
}
