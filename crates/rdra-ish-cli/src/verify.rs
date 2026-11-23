//! `rdra-ish verify --backend tlc`: check every invariant, after-assert, and temporal property
//! on its own cone, and a temporal property on the whole system only when its cone does not
//! decide it.
//!
//! A cone (`TransitionSystem::target_cone`) is closed: its restricted system reaches exactly
//! the projection of the whole system's states, so a safety verdict on the cone is the
//! verdict on the whole model. A property's cone (`property_cone`) also keeps the actions its
//! `.fair` names whole, so their weak fairness restricts to it; the whole model's
//! `WF_vars(Next)` does not, which is what `property_cone::Verdict` decides. Each property
//! runs alone, because TLC does not say which property its lasso violates.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use rdra_ish_core::lifecycle::{
    Bounds, BoundsLine, Caps, Cone, LtlKind, NotChecked, Scope, Selection, Severity, StateDiag,
    Target, TransitionSystem, VarSet,
};
use rdra_ish_core::{in_parallel, jobs, ResolvedProgram, SemanticModel};
use rdra_ish_emit::tla::{self, PrintMode, TlaBundle, TlaNames};

use crate::export::{checked_system, tla_bounds, tla_file, Modules};
use crate::property_cone::{AfterPlain, ConeRun, PropertyCone, Verdict};
use crate::prove::{find_z3, run_proof, Marks, ProofTally};
use crate::state_findings::{render_violation, Finding, TraceSource};
use crate::tlc_env::{tlc_env, Heap};
use crate::tlc_output::{progress, summarize, Outcome, TlcRun};
use crate::tlc_replay::{explain_property, explain_violation};

mod runner;
use runner::{run_cones, run_wholes, undecided_hint, write_spec};

/// Distinct states a whole-model temporal run may reach before it is stopped. Separate from
/// `Caps::VERIFY`, the BFS cap per cone: TLC's memory and time grow with this one, and
/// `--temporal-budget` bounds only the time.
const WHOLE_MODEL_STATE_CAP: u64 = 1 << 21;

/// What `verify` spends on a check it may not finish, and whether one it leaves undecided
/// fails the run.
pub struct Budget {
    /// Seconds a temporal property's whole-model run gets.
    pub temporal: Duration,
    /// `--deny-undecided`: an undecided finding is an error, not a warning.
    pub deny_undecided: bool,
    /// `--prove`: z3 tries each state rule for any number of rows.
    pub prove: Option<Prove>,
}

/// `verify --prove`.
pub struct Prove {
    /// `--z3`: the binary; else `RDRA_Z3`, else `z3` on PATH.
    pub z3: Option<PathBuf>,
    /// `--deny-unproved`: a rule left unproved fails the run.
    pub deny_unproved: bool,
}

/// A printed module and its cfg.
struct Spec {
    module: String,
    tla: String,
    cfg: String,
}

/// One TLC run: a module, its cfg, and the checks it answers.
struct Job {
    module: String,
    tla: String,
    cfg: String,
    checks: Vec<String>,
    kind: JobKind,
}

/// What a run's counterexamples replay on.
enum JobKind {
    Cone(Box<ConeSystem>),
    /// `properties()[i]` on its cone.
    PropertyCone(Box<PropertyJob>),
    /// `properties()[i]` on the whole system, after its cone left it undecided.
    Whole(usize),
}

/// A property on its cone; the `_Full` module beside it is the unreduced cone, run after a
/// counterexample on the reduced one, and the `_WF` module adds `WF_vars(Next)` for the
/// second run an `eventually` or `leads_to` may need. `names` are the unreduced cone's, which
/// both of those print.
struct PropertyJob {
    index: usize,
    cone: PropertyCone,
    names: TlaNames,
    full: Option<Spec>,
    wf: Option<Spec>,
}

impl Job {
    fn heap(&self) -> Heap {
        match self.kind {
            JobKind::Cone(_) | JobKind::PropertyCone(_) => Heap::Cone,
            JobKind::Whole(_) => Heap::Temporal,
        }
    }

    /// A guess at how long the run takes, to start the long ones first: a temporal property
    /// before a safety cone, since TLC also searches its behavior graph, then the system with
    /// more variables.
    fn weight(&self) -> (bool, usize) {
        match &self.kind {
            JobKind::Cone(cone) => (false, cone.system.var_ids().count()),
            JobKind::PropertyCone(p) => (true, p.cone.reduced.var_ids().count()),
            JobKind::Whole(_) => (true, usize::MAX),
        }
    }
}

/// A cone as TLC checks it: the whole system's cone, its restricted system, and the names
/// its module prints.
struct ConeSystem {
    cone: Cone,
    system: TransitionSystem,
    names: TlaNames,
}

/// What a run decides about its checks.
enum Decision {
    Decided(Outcome),
    /// The property is not decided, and why. `capped`: the whole-model run stopped at the
    /// state cap, not at the time budget.
    Undecided {
        why: String,
        capped: bool,
    },
}

impl Decision {
    /// What the run adds to the verdict of the whole: an undecided property fails nothing.
    fn outcome(&self) -> Outcome {
        match self {
            Decision::Decided(outcome) => *outcome,
            Decision::Undecided { .. } => Outcome::Ok,
        }
    }
}

struct Ran {
    decision: Decision,
    /// The run whose counterexample is explained: the last one of the job.
    run: TlcRun,
    log: String,
    /// What the run line says after the state count.
    notes: Vec<String>,
}

impl Ran {
    fn line(&self, module: &str, checks: &[String]) -> String {
        let verdict = match &self.decision {
            Decision::Undecided { .. } => "not decided",
            Decision::Decided(outcome) => outcome.as_str(),
        };
        let mut inside: Vec<String> = self
            .run
            .distinct_states
            .map(|n| format!("{n} states"))
            .into_iter()
            .collect();
        inside.extend(self.notes.iter().cloned());
        let inside = match inside.is_empty() {
            true => String::new(),
            false => format!(" ({})", inside.join(", ")),
        };
        format!("{module} [{}]: {verdict}{inside}", checks.join(", "))
    }
}

/// Where the specs go, and the whole-model module name. `-o X.tla` writes next to `X.tla`
/// and names the whole-model module `X`; any other `-o` is a directory.
fn out_dir(out: Option<PathBuf>) -> Result<(PathBuf, String, bool)> {
    let (dir, whole, keep) = match out {
        Some(path) => match tla_file(&path) {
            Some((parent, stem)) => (parent, stem, true),
            None => (path, "RdraSpec".to_string(), true),
        },
        None => (
            std::env::temp_dir().join(format!("rdra-ish-tlc-{}", std::process::id())),
            "RdraSpec".to_string(),
            false,
        ),
    };
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    // TLC runs inside `dir`, so the metadir and java tmpdir paths must not be relative.
    let dir = dunce::canonicalize(&dir)
        .with_context(|| format!("failed to resolve {}", dir.display()))?;
    Ok((dir, whole, keep))
}

/// Each distinct cone of a selected invariant or after-assert, in target order, named after
/// the first check that reads it. Cones over the same vars share one run: an action kept for
/// an after-assert writes nothing in the cone, so it adds edges but no states.
fn cone_jobs(
    ts: &TransitionSystem,
    names: &TlaNames,
    targets: &[Target],
    modules: &mut Modules,
) -> Vec<Job> {
    let mut at: BTreeMap<VarSet, usize> = BTreeMap::new();
    let mut order: Vec<(Cone, String, Vec<Target>)> = Vec::new();
    for &target in targets {
        let first = match target {
            Target::Entity(_) => continue,
            Target::Invariant(i) => names.invariant(i).to_string(),
            Target::AfterAssert(i) => names.after(i).to_string(),
        };
        let cone = ts.target_cone(target);
        match at.get(&cone.vars) {
            Some(&k) => {
                order[k].0.keep.extend(cone.keep);
                order[k].2.push(target);
            }
            None => {
                at.insert(cone.vars.clone(), order.len());
                order.push((cone, first, vec![target]));
            }
        }
    }
    // The names are claimed in target order; reducing and printing a cone reads nothing of
    // another, so the cones are prepared side by side.
    let named: Vec<(Cone, String, Vec<Target>)> = order
        .into_iter()
        .map(|(cone, first, shared)| (cone, modules.claim(&format!("Cone_{first}")), shared))
        .collect();
    in_parallel(named, jobs(), |(cone, module, shared)| {
        let seeds = shared.iter().map(|t| ts.target_seed(*t));
        let sub = ts.reduced_cone(&cone, seeds, Caps::VERIFY);
        let bundle = tla::print(&sub, PrintMode::Safety, &module);
        let n = &bundle.names;
        let checks = (0..sub.invariants().len())
            .map(|i| n.invariant(i).to_string())
            .chain((0..sub.after_asserts().len()).map(|i| n.after(i).to_string()))
            .collect();
        Job {
            module: bundle.module_name,
            tla: bundle.tla,
            cfg: bundle.cfg,
            checks,
            kind: JobKind::Cone(Box::new(ConeSystem {
                cone,
                system: sub,
                names: bundle.names,
            })),
        }
    })
}

/// One run per temporal property on its cone, module `Cone_<property>`, with the `_WF`
/// module (the same cone with `WF_vars(Next)`) printed beside it for the second run. Every
/// `Cone_<property>` is claimed before any `_Full` or `_WF`, so each property's own module
/// keeps its name.
fn property_jobs(ts: &TransitionSystem, names: &TlaNames, modules: &mut Modules) -> Vec<Job> {
    let plain: Vec<String> = (0..ts.properties().len())
        .map(|i| modules.claim(&format!("Cone_{}", names.property(i))))
        .collect();
    // Reducing a property's cone reads nothing of another's; the `_Full` and `_WF` names are
    // claimed afterwards, in property order.
    let cones = in_parallel((0..plain.len()).collect(), jobs(), |i| {
        PropertyCone::new(ts, i)
    });
    plain
        .into_iter()
        .zip(cones)
        .enumerate()
        .map(|(i, (module, cone))| {
            let name = names.property(i);
            let plain = cone.bundle(ConeRun::Plain, &module);
            let spec = |b: TlaBundle| Spec {
                module: b.module_name,
                tla: b.tla,
                cfg: b.cfg,
            };
            let mut names = plain.names;
            let full = cone.is_reduced().then(|| {
                let b = cone.bundle(ConeRun::Full, &modules.claim(&format!("{module}_Full")));
                names = b.names.clone();
                spec(b)
            });
            let wf = (cone.kind != LtlKind::Always).then(|| {
                let module = modules.claim(&format!("{module}_WF"));
                spec(cone.bundle(ConeRun::WfNext, &module))
            });
            Job {
                module: plain.module_name,
                tla: plain.tla,
                cfg: plain.cfg,
                checks: vec![name.to_string()],
                kind: JobKind::PropertyCone(Box::new(PropertyJob {
                    index: i,
                    cone,
                    names,
                    full,
                    wf,
                })),
            }
        })
        .collect()
}

/// `properties()[i]` on the whole system, module `module` (`<whole>_<property>`): the same
/// module text as the whole-model module, fairness included, and a cfg that names the one
/// property.
fn whole_job(ts: &TransitionSystem, names: &TlaNames, module: &str, i: usize) -> Job {
    let name = names.property(i);
    let bundle = tla::print(ts, PrintMode::Property(i), module);
    Job {
        module: bundle.module_name,
        tla: bundle.tla,
        cfg: bundle.cfg,
        checks: vec![name.to_string()],
        kind: JobKind::Whole(i),
    }
}

/// Each violated check of a cone run once, in model terms, from its first TLC trace. A trace
/// that does not replay is a disagreement between the engines and says so.
fn model_traces(
    program: &ResolvedProgram,
    model: &SemanticModel,
    (ts, names): (&TransitionSystem, &TlaNames),
    job: &JobKind,
    run: &TlcRun,
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let explained: Vec<Result<StateDiag, String>> = match job {
        JobKind::Cone(cs) => run
            .violations
            .iter()
            .filter(|v| v.name.is_some() && seen.insert(&v.name))
            .map(|v| {
                let mut diag = explain_violation(&cs.system, &cs.names, v)?;
                if let Some(trace) = diag.trace_mut() {
                    ts.annotate(&cs.cone, trace);
                }
                Ok(diag)
            })
            .collect(),
        JobKind::PropertyCone(p) => run
            .violations
            .iter()
            .map(|v| explain_property(&p.cone.system, &p.names, p.cone.index, v))
            .collect(),
        JobKind::Whole(i) => run
            .violations
            .iter()
            .map(|v| explain_property(ts, names, *i, v))
            .collect(),
    };
    explained
        .into_iter()
        .map(|explained| match explained {
            Ok(diag) => {
                let message = render_violation(program, model, &diag, TraceSource::Tlc)
                    .expect("explain returns a violation");
                Finding::new(&diag, &[], message).to_string()
            }
            Err(why) => {
                format!("error: {why}; BFS and TLC disagree (a tool bug). Raw TLC trace follows.")
            }
        })
        .collect()
}

/// Runs `verify --backend tlc`. `bounds` is `(--instance-count, --int-range-max)`; `tlc_jar`
/// is `--tlc-jar`.
pub fn run_tlc(
    program: &ResolvedProgram,
    model: &SemanticModel,
    out: Option<PathBuf>,
    (instance_count, int_range_max): (u32, u32),
    scope: &Scope,
    Budget {
        temporal: budget,
        deny_undecided,
        prove,
    }: Budget,
    tlc_jar: Option<PathBuf>,
) -> Result<()> {
    let bounds = tla_bounds(instance_count, int_range_max)?;
    let built = checked_system(model, bounds, scope)?;
    let (ts, selection) = (built.ts, built.selection);
    let (dir, whole_name, keep) = out_dir(out)?;
    let names = TlaNames::new(&ts);

    // A scope checks no property: `--buc` drops use cases fairness would read, and an error
    // `--entity` defers may sit in a property's cone.
    let temporal = !ts.properties().is_empty() && scope.is_whole();
    // The whole-model module keeps the name `-o` gives it; the generated names yield.
    let mut modules = Modules::default();
    let whole_stem = whole_name.clone();
    let whole_name = match temporal {
        true => modules.claim(&whole_name),
        false => whole_name,
    };
    // A `.lemma()` rule only helps `--prove`, which keeps a lemma only when it proves it, so
    // TLC runs no cone for one: a lemma that joins two large cones would cost more than the
    // rules it helps. `check` still checks it.
    let lemmas = rdra_ish_core::lifecycle::prove::lemma_invariants(model, &ts);
    let targets: Vec<Target> = selection
        .targets
        .iter()
        .copied()
        .filter(|t| !matches!(t, Target::Invariant(i) if lemmas.contains(i)))
        .collect();
    let mut jobs = cone_jobs(&ts, &names, &targets, &mut modules);
    let cones = jobs.len();
    let line = print_plan(
        model,
        &ts,
        scope,
        &selection,
        bounds,
        (cones, temporal),
        budget,
    );
    // The whole-model module with every property, for a run by hand; verify runs a property
    // on it only when its cone leaves it undecided.
    let whole = temporal.then(|| tla::print(&ts, PrintMode::Temporal, &whole_name));
    let mut whole_modules = Vec::new();
    if temporal {
        jobs.extend(property_jobs(&ts, &names, &mut modules));
        whole_modules = (0..ts.properties().len())
            .map(|i| modules.claim(&format!("{whole_stem}_{}", names.property(i))))
            .collect();
    }
    // TLC starts on the cones while BFS and the proof run here: none reads what another
    // finds. A failure to write the specs or to find a tool is reported where it was before
    // the engines ran side by side, after what BFS and the proof print, and starts no TLC run.
    let written = write_specs_and_index(&dir, &jobs, &whole);
    let tlc = (!jobs.is_empty()).then(|| Tlc::find(tlc_jar));
    let z3 = prove.as_ref().map(|p| find_z3(p.z3.clone()));
    let start = match (&written, &tlc, &z3) {
        (Ok(_), Some(Ok(tlc)), None | Some(Ok(_))) => Some(tlc),
        _ => None,
    };
    let (bfs_errors, capped, proof, cones_ran) = std::thread::scope(|threads| {
        let cones = start.map(|tlc| threads.spawn(|| run_cones(tlc, &dir, &modules, &jobs)));
        let (bfs_errors, capped) = state_models(program, model, &ts, scope, &selection);
        let proof = match (&prove, z3) {
            (Some(Prove { deny_unproved, .. }), Some(z3)) => z3.and_then(|z3| {
                let marks = Marks::of(model, &ts);
                run_proof(&ts, &selection, &marks, &z3, *deny_unproved).map(Some)
            }),
            _ => Ok(None),
        };
        let cones_ran = cones.map(|cones| match cones.join() {
            Ok(ran) => ran,
            Err(panic) => std::panic::resume_unwind(panic),
        });
        (bfs_errors, capped, proof, cones_ran)
    });
    let end = End {
        bfs_errors,
        capped,
        deny_undecided,
        proof: proof?,
    };

    let (mut index, index_path) = written?;
    if keep {
        println!(
            "wrote {} specs and {}",
            jobs.len() + usize::from(whole.is_some()),
            index_path.display()
        );
    }

    if jobs.is_empty() {
        if !keep {
            let _ = fs::remove_dir_all(&dir);
        }
        println!("nothing to check");
        println!("tlc_result: ok");
        return end.finish(&line, &[]);
    }
    let tlc = tlc.expect("a run with jobs looks for TLC")?;
    let mut ran = cones_ran.expect("TLC started once the specs and the tools were there")?;
    for (job, r) in jobs.iter().zip(&ran) {
        println!("{}", r.line(&job.module, &job.checks));
    }

    let undecided = run_undecided_wholes(
        (&tlc, dir.as_path()),
        (&ts, &names),
        &whole_modules,
        budget,
        (&mut jobs, &mut ran),
        (&mut index, index_path.as_path()),
    )?;

    let outcome = if ran
        .iter()
        .any(|r| r.decision.outcome() == Outcome::Counterexample)
    {
        Outcome::Counterexample
    } else if ran
        .iter()
        .any(|r| r.decision.outcome() == Outcome::ToolError)
    {
        Outcome::ToolError
    } else {
        Outcome::Ok
    };
    if outcome == Outcome::Ok {
        if !keep {
            let _ = fs::remove_dir_all(&dir);
        }
        println!("tlc_result: ok");
        return end.finish(&line, &undecided);
    }
    fail(
        program,
        model,
        (&ts, &names),
        (dir.as_path(), keep),
        (jobs.as_slice(), ran.as_slice()),
        outcome,
    )
}

/// The header of a run: the bounds line, the cone and property counts, and the fairness
/// assumptions. Returns the bounds line for the closing summary.
fn print_plan(
    model: &SemanticModel,
    ts: &TransitionSystem,
    scope: &Scope,
    selection: &Selection,
    bounds: Bounds,
    (cones, temporal): (usize, bool),
    budget: Duration,
) -> BoundsLine {
    let line =
        BoundsLine::verify(bounds, Some(Caps::VERIFY)).with_scope(scope.line(model, selection));
    println!("{line}");
    let checked = |pick: fn(&Target) -> bool| selection.targets.iter().filter(|t| pick(t)).count();
    println!(
        "cones: {cones} for {} invariant(s) and {} after-assert(s)",
        checked(|t| matches!(t, Target::Invariant(_))),
        checked(|t| matches!(t, Target::AfterAssert(_)))
    );
    match (temporal, ts.properties().len()) {
        (_, 0) => println!("temporal properties: none"),
        (true, n) => println!(
            "temporal properties: {n}, each checked on its cone, and on the whole model only when the cone does not decide it (budget {} s)",
            budget.as_secs()
        ),
        (false, n) => println!("temporal properties: {n}, not checked: a scoped run checks none"),
    }
    // A verdict under `.fair(...)` holds only on the runs that do not starve those use cases.
    for p in ts.properties().iter().filter(|p| !p.fair.is_empty()) {
        let ucs: Vec<&str> = p
            .fair
            .iter()
            .map(|uk| model.use_cases[*uk].id.as_str())
            .collect();
        println!(
            "  {} assumes weak fairness per instance on {}",
            p.name,
            ucs.join(", ")
        );
    }
    line
}

/// Writes every job's specs, the whole-model module, and `cones.txt` under `dir`. Returns the
/// index text and its path.
fn write_specs_and_index(
    dir: &Path,
    jobs: &[Job],
    whole: &Option<TlaBundle>,
) -> Result<(String, PathBuf)> {
    let mut index = String::new();
    for job in jobs {
        write_spec(dir, &job.module, &job.tla, &job.cfg)?;
        index.push_str(&format!("{}.tla: {}\n", job.module, job.checks.join(" ")));
        if let JobKind::PropertyCone(p) = &job.kind {
            for extra in p.full.iter().chain(&p.wf) {
                write_spec(dir, &extra.module, &extra.tla, &extra.cfg)?;
                index.push_str(&format!("{}.tla: {}\n", extra.module, job.checks.join(" ")));
            }
        }
    }
    if let Some(w) = &whole {
        write_spec(dir, &w.module_name, &w.tla, &w.cfg)?;
    }
    let index_path = dir.join("cones.txt");
    fs::write(&index_path, &index)
        .with_context(|| format!("failed to write {}", index_path.display()))?;
    Ok((index, index_path))
}

/// Runs the properties the cones leave undecided on the whole model, under the budget, and
/// appends their jobs and results. Returns the findings the whole-model runs leave undecided,
/// one line each.
fn run_undecided_wholes(
    (tlc, dir): (&Tlc, &Path),
    (ts, names): (&TransitionSystem, &TlaNames),
    whole_modules: &[String],
    budget: Duration,
    (jobs, ran): (&mut Vec<Job>, &mut Vec<Ran>),
    (index, index_path): (&mut String, &Path),
) -> Result<Vec<String>> {
    // The properties the cones leave undecided run on the whole model, under the budget.
    let wholes: Vec<Job> = jobs
        .iter()
        .zip(&*ran)
        .filter(|(_, r)| matches!(r.decision, Decision::Undecided { .. }))
        .filter_map(|(job, _)| match &job.kind {
            JobKind::PropertyCone(p) => {
                Some(whole_job(ts, names, &whole_modules[p.index], p.index))
            }
            _ => None,
        })
        .collect();
    for job in &wholes {
        write_spec(dir, &job.module, &job.tla, &job.cfg)?;
        index.push_str(&format!("{}.tla: {}\n", job.module, job.checks.join(" ")));
    }
    if !wholes.is_empty() {
        fs::write(index_path, &index)
            .with_context(|| format!("failed to write {}", index_path.display()))?;
        let more = run_wholes(tlc, dir, &wholes, budget)?;
        for (job, r) in wholes.iter().zip(&more) {
            println!("{}", r.line(&job.module, &job.checks));
        }
        let undecided = undecided_lines(&wholes, &more);
        jobs.extend(wholes);
        ran.extend(more);
        return Ok(undecided);
    }
    Ok(Vec::new())
}

/// The findings the whole-model runs leave undecided, one line each.
fn undecided_lines(jobs: &[Job], ran: &[Ran]) -> Vec<String> {
    jobs.iter()
        .zip(ran)
        .filter_map(|(job, r)| {
            let Decision::Undecided { why, capped } = &r.decision else {
                return None;
            };
            let states = r
                .run
                .distinct_states
                .map_or(String::new(), |n| format!(" after {n} states"));
            Some(format!(
                "  {}: the whole-model run {why}{states}; {}",
                job.checks.join(", "),
                undecided_hint(*capped)
            ))
        })
        .collect()
}

/// Reports the failed jobs: their lifted counterexamples and TLC's summary. Always an error.
fn fail(
    program: &ResolvedProgram,
    model: &SemanticModel,
    (ts, names): (&TransitionSystem, &TlaNames),
    (dir, keep): (&Path, bool),
    (jobs, ran): (&[Job], &[Ran]),
    outcome: Outcome,
) -> Result<()> {
    eprintln!("tlc_result: {}", outcome.as_str());
    let failed: Vec<String> = jobs
        .iter()
        .zip(ran)
        .filter(|(_, r)| r.decision.outcome() == outcome)
        .map(|(job, r)| {
            let mut body = format!(
                "{} ({}):\n",
                job.module,
                dir.join(format!("{}.tla", job.module)).display()
            );
            if r.decision.outcome() == Outcome::Counterexample {
                for block in model_traces(program, model, (ts, names), &job.kind, &r.run) {
                    body.push_str(&block);
                    body.push('\n');
                }
            }
            body.push_str(&summarize(&r.run, &r.log));
            body
        })
        .collect();
    let specs = if keep {
        format!("specs: {}", dir.display())
    } else {
        format!("specs: {} (kept because a check failed)", dir.display())
    };
    match outcome {
        Outcome::Counterexample => bail!("TLC verification failed\n{}\n{specs}", failed.join("\n")),
        _ => bail!(
            "TLC tool failed without a model violation\n{}\n{specs}",
            failed.join("\n")
        ),
    }
}

/// BFS on each selected entity cone at the verify bounds: the `NoWitness` errors, the
/// `Deadlock`s (errors, or warnings at the bound), and the `NeverRuns` warnings, rendered,
/// then the `not checked:` block and one summary line. Returns the errors and the findings a
/// cone left undecided at the state cap.
fn state_models(
    program: &ResolvedProgram,
    model: &SemanticModel,
    ts: &TransitionSystem,
    scope: &Scope,
    selection: &Selection,
) -> (usize, usize) {
    let entities: Vec<Target> = selection
        .targets
        .iter()
        .copied()
        .filter(|t| matches!(t, Target::Entity(_)))
        .collect();
    let mut run = ts.explore_targets_reduced(&entities, Caps::VERIFY);
    if !scope.bucs.is_empty() {
        run.defer_progress();
    }
    // A finding `--buc` leaves open is the scope's choice, not a bound's.
    let capped = run.undecided().iter().filter(|u| u.cone.is_some()).count();
    let not_checked = NotChecked {
        ts,
        model,
        scope,
        selection,
        undecided: run.undecided(),
        temporal: ts.properties().len(),
    };
    if !not_checked.is_empty() {
        println!("{not_checked}");
    }
    let mut errors = 0;
    for d in run.verdicts() {
        if let Some(text) = render_violation(program, model, &d.diag, TraceSource::Bfs) {
            let finding = Finding::new(&d.diag, &[], text);
            eprintln!("{finding}");
            errors += usize::from(finding.severity == Severity::Error);
        }
    }
    let t = run.tally();
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    println!(
        "state models (BFS on {}): {}, {} witnessed, {}, {}",
        plural(t.entity_cones, "entity cone", "entity cones"),
        plural(t.state_models, "state model", "state models"),
        plural(t.witnessed, "state", "states"),
        plural(t.deadlocks, "deadlock", "deadlocks"),
        plural(
            t.never_run,
            "use case that never runs",
            "use cases that never run"
        ),
    );
    (errors, capped)
}

/// How a run whose TLC checks passed ends.
struct End {
    bfs_errors: usize,
    /// Findings a BFS cone left undecided at the state cap.
    capped: usize,
    deny_undecided: bool,
    /// What `--prove` proved, when it ran.
    proof: Option<ProofTally>,
}

impl End {
    /// A BFS error still fails the run. A temporal property the budget left undecided is
    /// listed, as the BFS cap lists the findings it leaves; what the bounds left undecided is
    /// a warning, and an error under `--deny-undecided`.
    fn finish(&self, line: &BoundsLine, undecided: &[String]) -> Result<()> {
        if !undecided.is_empty() {
            println!(
                "not checked: {} not decided",
                match undecided.len() {
                    1 => "1 temporal property".to_string(),
                    n => format!("{n} temporal properties"),
                }
            );
            for u in undecided {
                println!("{u}");
            }
        }
        if self.bfs_errors > 0 {
            bail!(
                "verification failed: {} state-model error(s) from BFS on the entity cones",
                self.bfs_errors
            );
        }
        let open = [
            (
                self.capped,
                "finding a cone stopped at the state cap left",
                "findings a cone stopped at the state cap left",
            ),
            (
                undecided.len(),
                "temporal property the budget left",
                "temporal properties the budget left",
            ),
        ]
        .iter()
        .filter(|(n, ..)| *n > 0)
        .map(|(n, one, many)| format!("{n} {}", if *n == 1 { one } else { many }))
        .collect::<Vec<_>>();
        if !open.is_empty() {
            let what = format!("{} undecided", open.join(" and "));
            if self.deny_undecided {
                bail!("verification incomplete: {what} (--deny-undecided)");
            }
            eprintln!("warning: {what}; the OK line holds only for what was decided (--deny-undecided fails the run)");
        }
        match self.proof {
            Some(p) if p.unproved > 0 && p.deny_unproved => {
                bail!(
                    "verification incomplete: {} of {} rules unproved for any number of rows and any Int value (--deny-unproved)",
                    p.unproved,
                    p.rules
                );
            }
            Some(p) => println!("{}; {}", line.ok(), p.clause()),
            None => println!("{}", line.ok()),
        }
        Ok(())
    }
}

/// How `verify` starts TLC: a `tlc` / `tlc2` launcher on PATH, or `java -cp <jar> tlc2.TLC`
/// when `--tlc-jar` or `RDRA_TLC_JAR` names `tla2tools.jar`.
pub(crate) enum Tlc {
    Launcher(PathBuf),
    Jar(PathBuf),
}

/// The env variable that names `tla2tools.jar` when `--tlc-jar` is not given.
pub(crate) const TLC_JAR_ENV: &str = "RDRA_TLC_JAR";

impl Tlc {
    /// `--tlc-jar` first, then `RDRA_TLC_JAR`, then `tlc` and `tlc2` on PATH.
    fn find(jar: Option<PathBuf>) -> Result<Tlc> {
        let jar = jar.or_else(|| {
            std::env::var_os(TLC_JAR_ENV)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        });
        if let Some(jar) = jar {
            if !jar.is_file() {
                bail!(
                    "tla2tools.jar not found at {} (from --tlc-jar or {TLC_JAR_ENV})",
                    jar.display()
                );
            }
            return Ok(Tlc::Jar(jar));
        }
        for candidate in ["tlc", "tlc2"] {
            if let Some(path) = find_on_path(candidate) {
                return Ok(Tlc::Launcher(path));
            }
        }
        bail!(
            "TLC not found on PATH (tried `tlc`, `tlc2`)\n\
             Get it one of these ways:\n  \
             - Nix: `nix shell nixpkgs#tlaplus` (add `--extra-experimental-features 'nix-command flakes'` \
             if flakes are off), then run verify in that shell\n  \
             - Download tla2tools.jar from https://github.com/tlaplus/tlaplus/releases and pass \
             `--tlc-jar <path>` or set {TLC_JAR_ENV}=<path> (needs `java` on PATH)\n\
             Or use `rdra-ish export --kind tla` to generate the spec without running TLC."
        );
    }

    fn command(&self) -> Command {
        match self {
            Tlc::Launcher(path) => Command::new(path),
            Tlc::Jar(jar) => {
                let mut command = Command::new("java");
                command.arg("-cp").arg(jar).arg("tlc2.TLC");
                command
            }
        }
    }

    fn display(&self) -> String {
        match self {
            Tlc::Launcher(path) => path.display().to_string(),
            Tlc::Jar(jar) => format!("java -cp {} tlc2.TLC", jar.display()),
        }
    }
}

/// The first `name` on PATH that is a file, also trying the platform's executable suffix.
pub(crate) fn find_on_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    let exe = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(&paths).find_map(|dir| {
        [name, exe.as_str()]
            .into_iter()
            .map(|file| dir.join(file))
            .find(|path| path.is_file())
    })
}
