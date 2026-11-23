//! Run every TLC check without enumerating duplicate counterexamples.

use super::*;

/// One TLC run of `job`; a cone is rerun without reported checks until all have failed
/// or the remaining checks pass. Each run stops at its first counterexample instead of
/// enumerating repeated violations of the same check. The reruns join every distinct check.
/// A property cone run that finds a counterexample is followed by the `_WF` run when the
/// verdict asks for it (`property_cone`).
fn run_job(tlc: &Tlc, dir: &Path, modules: &Modules, job: &Job, workers: &str) -> Result<Ran> {
    let cfg = format!("{}.cfg", job.module);
    let safety = matches!(job.kind, JobKind::Cone(_));
    // With several workers TLC reaches the same states, so an `ok` is the `ok` of one
    // worker, but it no longer finds a violation by the shortest trace, nor the same one on
    // every run. A safety cone that does not pass is therefore run again on one worker, and
    // only that run is reported. A property's lasso differs from run to run on one worker too.
    let workers = match (safety, workers) {
        (true, many) if many != "1" => {
            let ran = run_cfg(tlc, dir, &job.module, job.heap(), &cfg, many)?;
            if ran.decision.outcome() == Outcome::Ok {
                return Ok(ran);
            }
            "1"
        }
        _ => workers,
    };
    let mut ran = run_cfg(tlc, dir, &job.module, job.heap(), &cfg, workers)?;
    match &job.kind {
        JobKind::Cone(cs) => {
            let mut reruns = 0;
            while ran.decision.outcome() == Outcome::Counterexample {
                let found = ran.run.violated_names();
                if found.len() >= job.checks.len() {
                    break;
                }
                reruns += 1;
                let cfg_name = modules.rest_cfg(&job.module, reruns);
                let cfg = tla::cfg_without(&cs.system, &cs.names, PrintMode::Safety, &found);
                let path = dir.join(&cfg_name);
                fs::write(&path, cfg)
                    .with_context(|| format!("failed to write {}", path.display()))?;
                let more = run_cfg(tlc, dir, &job.module, job.heap(), &cfg_name, workers)?;
                let outcome = more.decision.outcome();
                ran.log.push_str(&more.log);
                ran.run.violations.extend(more.run.violations);
                if outcome == Outcome::ToolError {
                    ran.decision = Decision::Decided(Outcome::ToolError);
                    break;
                }
                if ran.run.violated_names().len() == found.len() {
                    break;
                }
            }
        }
        JobKind::PropertyCone(p) => {
            let (cone, wf) = (&p.cone, &p.wf);
            let mut plain = ran.decision.outcome();
            // A counterexample on the reduced cone is real, but its lasso is read on the
            // unreduced cone, whose run gives the verdict from here on.
            if let (Outcome::Counterexample, Some(full)) = (plain, &p.full) {
                let cfg = format!("{}.cfg", full.module);
                let more = run_cfg(tlc, dir, &full.module, job.heap(), &cfg, workers)?;
                ran.notes.push(format!(
                    "{}: {}",
                    full.module,
                    more.decision.outcome().as_str()
                ));
                ran.log.push_str(&more.log);
                ran.run = more.run;
                plain = match more.decision.outcome() {
                    Outcome::Ok => {
                        ran.notes.push(
                            "the reduced cone finds a counterexample the cone does not (a tool bug)"
                                .to_string(),
                        );
                        Outcome::ToolError
                    }
                    other => other,
                };
            }
            let verdict = match cone.after_plain(plain) {
                AfterPlain::Decided(verdict) => verdict,
                AfterPlain::NeedsWfNext => {
                    let wf = wf
                        .as_ref()
                        .expect("an eventually or leads_to has a WF module");
                    let cfg = format!("{}.cfg", wf.module);
                    let more = run_cfg(tlc, dir, &wf.module, job.heap(), &cfg, workers)?;
                    let wf_outcome = more.decision.outcome();
                    ran.notes
                        .push(format!("{}: {}", wf.module, wf_outcome.as_str()));
                    ran.log.push_str(&more.log);
                    ran.run = more.run;
                    cone.after_wf(plain, wf_outcome)
                }
            };
            ran.decision = match verdict {
                Verdict::Ok => Decision::Decided(Outcome::Ok),
                Verdict::Counterexample => Decision::Decided(Outcome::Counterexample),
                Verdict::ToolError => Decision::Decided(Outcome::ToolError),
                Verdict::Undecided(why) => {
                    ran.notes.push(why.as_str().to_string());
                    Decision::Undecided {
                        why: why.as_str().to_string(),
                        capped: false,
                    }
                }
            };
        }
        JobKind::Whole(_) => {}
    }
    Ok(ran)
}

fn tlc_command(tlc: &Tlc, dir: &Path, module: &str, heap: Heap, workers: &str) -> Result<Command> {
    let meta = dir.join(format!("{module}.states"));
    // TLC unpacks its standard modules into java.io.tmpdir; parallel runs must not share it.
    let tmp = dir.join(format!("{module}.tmp"));
    fs::create_dir_all(&tmp).with_context(|| format!("failed to create {}", tmp.display()))?;
    let (key, value) = tlc_env(heap, &tmp);
    let mut command = tlc.command();
    command
        .env(key, value)
        .args(["-workers", workers, "-metadir"])
        .arg(&meta)
        .current_dir(dir);
    Ok(command)
}

fn clean_up(dir: &Path, module: &str) {
    let _ = fs::remove_dir_all(dir.join(format!("{module}.states")));
    let _ = fs::remove_dir_all(dir.join(format!("{module}.tmp")));
}

/// Stop at the first counterexample; `run_job` reruns every remaining check separately.
fn run_cfg(
    tlc: &Tlc,
    dir: &Path,
    module: &str,
    heap: Heap,
    cfg: &str,
    workers: &str,
) -> Result<Ran> {
    let tla = format!("{module}.tla");
    let mut command = tlc_command(tlc, dir, module, heap, workers)?;
    let output = command
        .args(["-config", cfg, &tla])
        .output()
        .with_context(|| format!("failed to execute {}", tlc.display()))?;
    clean_up(dir, module);
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let run = TlcRun::parse(&log);
    Ok(Ran {
        decision: Decision::Decided(run.outcome(output.status.success())),
        run,
        log,
        notes: Vec::new(),
    })
}

/// What to change when a property's whole-model run stopped. A property without `.fair` rests
/// on the whole model's `WF_vars(Next)`; naming the use cases it relies on lets its cone decide
/// it. The state cap is fixed, so only a smaller model or a cone lifts it.
pub(super) fn undecided_hint(capped: bool) -> &'static str {
    if capped {
        "the model has more states than the cap; --temporal-budget does not lift it, so name the use cases the property relies on with .fair(UseCase, ...) to decide it on its cone, or shrink the model"
    } else {
        "raise --temporal-budget, or name the use cases the property relies on with .fair(UseCase, ...) to decide it on its cone"
    }
}

/// A whole-model property run under a budget: TLC's progress lines (about one a minute) print
/// as they come, and the run is killed past `budget` or past `WHOLE_MODEL_STATE_CAP` distinct states,
/// and left undecided unless TLC had already printed its verdict.
fn run_whole(tlc: &Tlc, dir: &Path, job: &Job, workers: &str, budget: Duration) -> Result<Ran> {
    let tla = format!("{}.tla", job.module);
    let cfg = format!("{}.cfg", job.module);
    let mut child = tlc_command(tlc, dir, &job.module, job.heap(), workers)?
        .args(["-config", &cfg, &tla])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to execute {}", tlc.display()))?;
    let (tx, rx) = mpsc::channel::<String>();
    let readers: Vec<_> = [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
    ]
    .into_iter()
    .flatten()
    .map(|stream| {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        })
    })
    .collect();
    drop(tx);
    let start = Instant::now();
    let cap = WHOLE_MODEL_STATE_CAP;
    let mut log = String::new();
    let mut stopped: Option<String> = None;
    let mut capped = false;
    let mut status = None;
    while status.is_none() {
        while let Ok(line) = rx.try_recv() {
            // TLC prints a last progress line with an empty queue when it finishes; the run
            // line reports that.
            if let Some(p) = progress(&line).filter(|p| p.queued > 0) {
                println!(
                    "  {}: {} distinct states, {} queued, {} s",
                    job.module,
                    p.distinct,
                    p.queued,
                    start.elapsed().as_secs()
                );
                if p.distinct > cap && stopped.is_none() {
                    stopped = Some(format!("stopped at the state cap of {cap} states"));
                    capped = true;
                    let _ = child.kill();
                }
            }
            log.push_str(&line);
            log.push('\n');
        }
        status = child.try_wait().context("failed to wait for TLC")?;
        if status.is_none() && start.elapsed() > budget && stopped.is_none() {
            stopped = Some(format!(
                "stopped at the time budget of {} s",
                budget.as_secs()
            ));
            let _ = child.kill();
        }
        if status.is_none() {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    for reader in readers {
        let _ = reader.join();
    }
    for line in rx.try_iter() {
        log.push_str(&line);
        log.push('\n');
    }
    clean_up(dir, &job.module);
    let run = TlcRun::parse(&log);
    let exit_ok = status.is_some_and(|s| s.success());
    // A kill that lands after TLC printed its verdict stops nothing that decides.
    let (decision, notes) = match (stopped, run.verdict()) {
        (Some(_), Some(outcome)) => (Decision::Decided(outcome), Vec::new()),
        (Some(why), None) => (
            Decision::Undecided {
                why: why.clone(),
                capped,
            },
            vec![why],
        ),
        (None, _) => (Decision::Decided(run.outcome(exit_ok)), Vec::new()),
    };
    Ok(Ran {
        decision,
        run,
        log,
        notes,
    })
}

/// Runs the cone jobs, up to 8 at once. Each run gets the cores divided by the runs at once,
/// and no fewer than 2 workers on 4 cores or more: most models have a few long cones and
/// many short ones, and once the short ones are done a long run on one worker would leave
/// the other cores idle.
pub(super) fn run_cones(
    tlc: &Tlc,
    dir: &Path,
    modules: &Modules,
    jobs: &[Job],
) -> Result<Vec<Ran>> {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    let at_once = cores.min(8).min(jobs.len()).max(1);
    let workers = cone_workers(cores, at_once).to_string();
    run_some(jobs, at_once, &workers, |job, workers| {
        run_job(tlc, dir, modules, job, workers)
    })
}

/// TLC workers per cone run when `at_once` runs share `cores`.
fn cone_workers(cores: usize, at_once: usize) -> usize {
    let floor = if cores >= 4 { 2 } else { 1 };
    (cores / at_once).max(floor)
}

/// Runs the whole-model property jobs, up to 2 at once (each takes the temporal heap) sharing
/// the cores.
pub(super) fn run_wholes(
    tlc: &Tlc,
    dir: &Path,
    jobs: &[Job],
    budget: Duration,
) -> Result<Vec<Ran>> {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    let at_once = jobs.len().clamp(1, 2);
    let workers = (cores / at_once).max(1).to_string();
    run_some(jobs, at_once, &workers, |job, workers| {
        run_whole(tlc, dir, job, workers, budget)
    })
}

/// Runs `jobs` on `threads` threads, each TLC run with `workers` workers, the heaviest first
/// (`Job::weight`). The results come back in job order.
fn run_some(
    jobs: &[Job],
    threads: usize,
    workers: &str,
    run: impl Fn(&Job, &str) -> Result<Ran> + Sync,
) -> Result<Vec<Ran>> {
    let mut order: Vec<usize> = (0..jobs.len()).collect();
    order.sort_by_key(|i| std::cmp::Reverse(jobs[*i].weight()));
    let next = AtomicUsize::new(0);
    let done: Mutex<Vec<(usize, Result<Ran>)>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..threads.min(jobs.len()) {
            s.spawn(|| {
                while let Some(&i) = order.get(next.fetch_add(1, Ordering::SeqCst)) {
                    let ran = run(&jobs[i], workers);
                    done.lock().expect("lock").push((i, ran));
                }
            });
        }
    });
    let mut done = done.into_inner().expect("lock");
    done.sort_by_key(|(i, _)| *i);
    done.into_iter().map(|(_, r)| r).collect()
}

pub(super) fn write_spec(dir: &Path, module: &str, tla: &str, cfg: &str) -> Result<()> {
    for (ext, text) in [("tla", tla), ("cfg", cfg)] {
        let path = dir.join(format!("{module}.{ext}"));
        fs::write(&path, text).with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod worker_tests {
    use super::cone_workers;

    /// The runs at once never get fewer workers than the cores divide into, and a long run
    /// keeps a second worker once the short ones are done.
    #[test]
    fn cone_runs_share_the_cores_and_keep_two_workers_on_four_cores() {
        assert_eq!(cone_workers(1, 1), 1);
        assert_eq!(cone_workers(2, 2), 1);
        assert_eq!(cone_workers(4, 4), 2);
        assert_eq!(cone_workers(4, 1), 4);
        assert_eq!(cone_workers(8, 8), 2);
        assert_eq!(cone_workers(16, 8), 2);
        assert_eq!(cone_workers(16, 2), 8);
    }
}

#[cfg(test)]
mod hint_tests {
    use super::undecided_hint;

    /// Raising the budget cannot lift the state cap, so that hint must not offer it.
    #[test]
    fn a_state_cap_hint_does_not_offer_the_budget() {
        let hint = undecided_hint(true);
        assert!(hint.contains("does not lift"), "{hint}");
        assert!(hint.contains(".fair(UseCase, ...)"), "{hint}");
        assert!(!hint.starts_with("raise --temporal-budget"), "{hint}");
    }

    #[test]
    fn a_time_budget_hint_offers_the_budget_and_fair() {
        let hint = undecided_hint(false);
        assert!(hint.starts_with("raise --temporal-budget"), "{hint}");
        assert!(hint.contains(".fair(UseCase, ...)"), "{hint}");
    }
}
