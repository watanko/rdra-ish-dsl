//! `rdra-ish codegen` (experimental): the model's domain types as code, on stdout.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rdra_ish_core::format_location;
use rdra_ish_core::lifecycle::{Bounds, Caps, Scope, Severity};
use rdra_ish_emit::codegen::{generate, CheckStatus, Lang, Options};

use crate::cli::CodegenLang;
use crate::load::load_clean;
use crate::state_findings::state_findings;

pub fn run_codegen(
    inputs: &[PathBuf],
    lang: CodegenLang,
    out: Option<&Path>,
    package: &str,
    allow_failing: bool,
    python_config: Option<&Path>,
) -> Result<()> {
    let python = match python_config {
        Some(path) => {
            if !matches!(lang, CodegenLang::Python) {
                anyhow::bail!("--python-config requires --lang py");
            }
            serde_json::from_slice(
                &fs::read(path).with_context(|| format!("failed to read {}", path.display()))?,
            )
            .with_context(|| {
                format!(
                    "invalid Python representation options in {}",
                    path.display()
                )
            })?
        }
        None => Default::default(),
    };
    let (program, model) = load_clean(inputs)?;
    // The state types rest on the rules `check` verifies, so the generator runs it first.
    let found = state_findings(&program, &model, &Scope::ALL, Bounds::STATES, Caps::STATES);
    let errors: Vec<String> = found
        .findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .map(|f| format!("{}: {}", f.name, one_line(&f.message)))
        .collect();
    let check = if errors.is_empty() {
        let line = found.bounds.to_string();
        CheckStatus::Passed {
            bounds: line.strip_prefix("bounds: ").unwrap_or(&line).to_string(),
        }
    } else if allow_failing {
        eprintln!(
            "note: `check` reports {} error(s); the state types narrow no column, and the obligations list the errors",
            errors.len()
        );
        CheckStatus::Failed { errors }
    } else {
        for finding in found
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
        {
            eprintln!("{finding}");
        }
        anyhow::bail!(
            "`check` reports {} error(s); refusing to generate code for a model it rejects (--allow-failing generates it anyway)",
            errors.len()
        );
    };
    let options = Options {
        python,
        lang: match lang {
            CodegenLang::Ts => Lang::TypeScript,
            CodegenLang::Rust => Lang::Rust,
            CodegenLang::Go => Lang::Go,
            CodegenLang::Python => Lang::Python,
            CodegenLang::Policy => Lang::Policy,
            CodegenLang::PyPolicy => Lang::PythonPolicy,
            CodegenLang::PySqlPolicy => Lang::PythonSqlPolicy,
        },
        package: package.to_string(),
        model_path: inputs
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(
                if matches!(lang, CodegenLang::PyPolicy | CodegenLang::PySqlPolicy) {
                    "\n"
                } else {
                    " "
                },
            ),
        version: env!("CARGO_PKG_VERSION").to_string(),
        check,
    };
    // Sites relative to the working directory, as the user named the inputs.
    let cwd = std::env::current_dir().ok();
    let site = |loc: &_| {
        let text = format_location(&program, loc)?;
        Some(
            match cwd
                .as_ref()
                .and_then(|d| text.strip_prefix(&format!("{}/", d.display())))
            {
                Some(relative) => relative.to_string(),
                None => text,
            },
        )
    };
    let code = generate(&model, &options, &site)?;
    match out {
        Some(path) => {
            fs::write(path, format!("{code}\n"))
                .with_context(|| format!("failed to write {}", path.display()))?;
            eprintln!("wrote {}", path.display());
        }
        None => println!("{code}"),
    }
    Ok(())
}

/// The first line of a finding's message: a counterexample follows it.
fn one_line(message: &str) -> String {
    message
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}
