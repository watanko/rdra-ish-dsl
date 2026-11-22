use super::*;
use clap::Parser;

#[test]
fn parses_changed_subcommands_and_options() {
    let cli = Cli::try_parse_from([
        "rdra-ish",
        "diagram",
        "sample.rdra",
        "--kind",
        "rdra",
        "--format",
        "mermaid",
        "--buc",
        "BucScheduling",
        "--node-kind",
        "usecase",
        "--edge-kind",
        "invokes",
        "--out",
        "out.mmd",
    ])
    .unwrap();

    match cli.command {
        Commands::Diagram {
            inputs,
            kind,
            format,
            buc,
            usecase,
            entity,
            node_kind,
            edge_kind,
            out,
            ..
        } => {
            assert_eq!(inputs, vec![PathBuf::from("sample.rdra")]);
            assert!(usecase.is_empty());
            assert!(entity.is_empty());
            assert!(matches!(kind, DiagramKind::Rdra));
            assert!(matches!(format, OutputFormat::Mermaid));
            assert_eq!(buc, vec!["BucScheduling"]);
            assert_eq!(node_kind, vec!["usecase"]);
            assert_eq!(edge_kind, vec!["invokes"]);
            assert_eq!(out, Some(PathBuf::from("out.mmd")));
        }
        _ => panic!("expected diagram command"),
    }

    let cli = Cli::try_parse_from([
        "rdra-ish",
        "list",
        "sample.rdra",
        "--kind",
        "requirement",
        "--format",
        "json",
    ])
    .unwrap();

    match cli.command {
        Commands::List {
            inputs,
            kind,
            format,
        } => {
            assert_eq!(inputs, vec![PathBuf::from("sample.rdra")]);
            assert!(matches!(kind, ListKind::Requirement));
            assert!(matches!(format, ListFormat::Json));
        }
        _ => panic!("expected list command"),
    }

    let cli = Cli::try_parse_from([
        "rdra-ish",
        "list",
        "sample.rdra",
        "--kind",
        "adr-impact",
        "--format",
        "csv",
    ])
    .unwrap();

    match cli.command {
        Commands::List {
            inputs,
            kind,
            format,
        } => {
            assert_eq!(inputs, vec![PathBuf::from("sample.rdra")]);
            assert!(matches!(kind, ListKind::AdrImpact));
            assert!(matches!(format, ListFormat::Csv));
        }
        _ => panic!("expected list command"),
    }

    let cli = Cli::try_parse_from(["rdra-ish", "lint", "sample.rdra", "--format", "csv"]).unwrap();

    match cli.command {
        Commands::Lint { inputs, format } => {
            assert_eq!(inputs, vec![PathBuf::from("sample.rdra")]);
            assert!(matches!(format, ListFormat::Csv));
        }
        _ => panic!("expected lint command"),
    }

    let cli = Cli::try_parse_from(["rdra-ish", "fmt", "sample.rdra", "--write"]).unwrap();

    match cli.command {
        Commands::Fmt {
            inputs,
            write,
            check,
        } => {
            assert_eq!(inputs, vec![PathBuf::from("sample.rdra")]);
            assert!(write);
            assert!(!check);
        }
        _ => panic!("expected fmt command"),
    }

    let cli = Cli::try_parse_from([
        "rdra-ish",
        "list",
        "sample.rdra",
        "--kind",
        "actor-inputs",
        "--format",
        "json",
    ])
    .unwrap();

    match cli.command {
        Commands::List {
            inputs,
            kind,
            format,
        } => {
            assert_eq!(inputs, vec![PathBuf::from("sample.rdra")]);
            assert!(matches!(kind, ListKind::BusinessInputs));
            assert!(matches!(format, ListFormat::Json));
        }
        _ => panic!("expected list command"),
    }

    let cli = Cli::try_parse_from([
        "rdra-ish",
        "csv",
        "sample.rdra",
        "--kind",
        "business-inputs",
        "--out",
        "inputs.csv",
    ])
    .unwrap();

    match cli.command {
        Commands::Csv {
            inputs, kind, out, ..
        } => {
            assert_eq!(inputs, vec![PathBuf::from("sample.rdra")]);
            assert!(matches!(kind, CsvKind::BusinessInputs));
            assert_eq!(out, PathBuf::from("inputs.csv"));
        }
        _ => panic!("expected csv command"),
    }

    let cli = Cli::try_parse_from([
        "rdra-ish",
        "export",
        "sample.rdra",
        "--kind",
        "dbml",
        "--out",
        "schema.dbml",
    ])
    .unwrap();

    match cli.command {
        Commands::Export {
            inputs, kind, out, ..
        } => {
            assert_eq!(inputs, vec![PathBuf::from("sample.rdra")]);
            assert!(matches!(kind, ExportKind::Dbml));
            assert_eq!(out, PathBuf::from("schema.dbml"));
        }
        _ => panic!("expected export command"),
    }

    let cli = Cli::try_parse_from([
        "rdra-ish",
        "export",
        "sample.rdra",
        "--kind",
        "sql",
        "--dialect",
        "sqlite",
    ])
    .unwrap();

    match cli.command {
        Commands::Export { kind, dialect, .. } => {
            assert!(matches!(kind, ExportKind::Sql));
            assert!(matches!(dialect, SqlDialect::Sqlite));
        }
        _ => panic!("expected export command"),
    }

    let cli = Cli::try_parse_from([
        "rdra-ish",
        "export",
        "sample.rdra",
        "--kind",
        "asyncapi",
        "--out",
        "events.json",
    ])
    .unwrap();

    match cli.command {
        Commands::Export {
            inputs, kind, out, ..
        } => {
            assert_eq!(inputs, vec![PathBuf::from("sample.rdra")]);
            assert!(matches!(kind, ExportKind::Asyncapi));
            assert_eq!(out, PathBuf::from("events.json"));
        }
        _ => panic!("expected export command"),
    }

    let cli = Cli::try_parse_from([
        "rdra-ish",
        "export",
        "sample.rdra",
        "--kind",
        "json-schema",
        "--out",
        "schemas.json",
    ])
    .unwrap();

    match cli.command {
        Commands::Export {
            inputs,
            kind,
            out,
            instance_count,
            int_range_max,
            ..
        } => {
            assert_eq!(inputs, vec![PathBuf::from("sample.rdra")]);
            assert!(matches!(kind, ExportKind::JsonSchema));
            assert_eq!(out, PathBuf::from("schemas.json"));
            assert_eq!(instance_count, 2);
            assert_eq!(int_range_max, 5);
        }
        _ => panic!("expected export command"),
    }

    let cli = Cli::try_parse_from([
        "rdra-ish",
        "export",
        "sample.rdra",
        "--kind",
        "mermaid-er",
        "--out",
        "er.mmd",
    ])
    .unwrap();

    match cli.command {
        Commands::Export { kind, out, .. } => {
            assert!(matches!(kind, ExportKind::MermaidEr));
            assert_eq!(out, PathBuf::from("er.mmd"));
        }
        _ => panic!("expected export command"),
    }

    let cli = Cli::try_parse_from([
        "rdra-ish",
        "export",
        "sample.rdra",
        "--kind",
        "tla",
        "--out",
        "/tmp/rdra-tla",
        "--instance-count",
        "3",
        "--int-range-max",
        "9",
    ])
    .unwrap();

    match cli.command {
        Commands::Export {
            kind,
            instance_count,
            int_range_max,
            ..
        } => {
            assert!(matches!(kind, ExportKind::Tla));
            assert_eq!(instance_count, 3);
            assert_eq!(int_range_max, 9);
        }
        _ => panic!("expected export command"),
    }

    let cli = Cli::try_parse_from([
        "rdra-ish",
        "verify",
        "sample.rdra",
        "--backend",
        "tlc",
        "--instance-count",
        "4",
        "--int-range-max",
        "7",
    ])
    .unwrap();

    match cli.command {
        Commands::Verify {
            instance_count,
            int_range_max,
            ..
        } => {
            assert_eq!(instance_count, 4);
            assert_eq!(int_range_max, 7);
        }
        _ => panic!("expected verify command"),
    }

    let cli = Cli::try_parse_from([
        "rdra-ish",
        "states",
        "sample.rdra",
        "--format",
        "type-script",
    ])
    .unwrap();
    match cli.command {
        Commands::States { format, .. } => {
            assert!(matches!(format, StatesFormat::TypeScript));
        }
        _ => panic!("expected states command"),
    }

    let cli = Cli::try_parse_from([
        "rdra-ish",
        "states",
        "sample.rdra",
        "--format",
        "typescript",
    ])
    .unwrap();
    match cli.command {
        Commands::States { format, .. } => {
            assert!(matches!(format, StatesFormat::TypeScript));
        }
        _ => panic!("expected states command"),
    }

    for fmt in ["python", "py"] {
        let cli =
            Cli::try_parse_from(["rdra-ish", "states", "sample.rdra", "--format", fmt]).unwrap();
        match cli.command {
            Commands::States { format, .. } => {
                assert!(matches!(format, StatesFormat::Python));
            }
            _ => panic!("expected states command"),
        }
    }

    for lang in ["py", "python"] {
        let cli =
            Cli::try_parse_from(["rdra-ish", "codegen", "sample.rdra", "--lang", lang]).unwrap();
        match cli.command {
            Commands::Codegen { lang, .. } => {
                assert!(matches!(lang, CodegenLang::Python));
            }
            _ => panic!("expected codegen command"),
        }
    }

    for kind in ["python-states", "py-states"] {
        let cli = Cli::try_parse_from([
            "rdra-ish",
            "export",
            "sample.rdra",
            "--kind",
            kind,
            "--out",
            "states.py",
        ])
        .unwrap();
        match cli.command {
            Commands::Export { kind, .. } => {
                assert!(matches!(kind, ExportKind::PythonStates));
            }
            _ => panic!("expected export command"),
        }
    }
}

fn parse_error(args: &[&str]) -> String {
    match Cli::try_parse_from(args) {
        Ok(_) => panic!("{args:?} parsed"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn removed_plantuml_values_name_their_replacement() {
    for format in ["puml", "svg", "png"] {
        let message = parse_error(&["rdra-ish", "diagram", "m.rdra", "--format", format]);
        assert!(
            message.contains(&format!("`--format {format}` is no longer supported"))
                && message.contains("`--format mermaid`"),
            "{message}"
        );
    }
    let message = parse_error(&["rdra-ish", "export", "m.rdra", "--kind", "plantuml-er"]);
    assert!(
        message.contains("`--kind plantuml-er` is no longer supported")
            && message.contains("`--kind mermaid-er`"),
        "{message}"
    );
}

#[test]
fn removed_diagram_kinds_name_their_replacement() {
    for (kind, replacement) in [
        ("boundaryless-graph", "`--kind rdra`"),
        ("business-area", "`csv --kind business-inputs`"),
        ("technical-area", "`csv --kind api-matrix`"),
        ("diff", "`git diff`"),
        ("event-flow", "`--kind event`"),
    ] {
        let message = parse_error(&["rdra-ish", "diagram", "m.rdra", "--kind", kind]);
        assert!(
            message.contains(&format!("`--kind {kind}` is no longer supported"))
                && message.contains(replacement),
            "{message}"
        );
    }
    let message = parse_error(&["rdra-ish", "export", "m.rdra", "--kind", "plantuml-er"]);
    assert!(
        message.contains("`--kind plantuml-er` is no longer supported")
            && message.contains("`--kind mermaid-er`"),
        "{message}"
    );
}

#[test]
fn an_unknown_value_keeps_the_generic_error() {
    let message = parse_error(&["rdra-ish", "diagram", "m.rdra", "--format", "gif"]);
    assert!(message.contains("invalid value 'gif'"), "{message}");
    assert!(message.contains("[possible values: mermaid]"), "{message}");
}
