use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::BuildTarget;
use std::path::{Path, PathBuf};

fn app(body: &str) -> roundhouse::App {
    let files = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"query_records\" do |t|\n    t.jsonb \"payload\"\n  end\nend\n".to_string()),
        ("app/models/query_record.rb", "class QueryRecord < ApplicationRecord\nend\n".to_string()),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/query\" => \"queries#index\"\nend\n".to_string()),
        ("app/controllers/queries_controller.rb", format!("class QueriesController < ApplicationController\n  def index\n    {body}\n  end\nend\n")),
    ].into_iter().map(|(path, source)| (PathBuf::from(path), source.into_bytes())).collect();
    let mut app = ingest_app_from_tree(files).unwrap();
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn source(app: &roundhouse::App) -> String {
    ruby::emit_spinel(app)
        .iter()
        .find(|file| file.path.ends_with("app/controllers/queries_controller.rb"))
        .unwrap()
        .content
        .clone()
}

#[test]
fn callback_hash_goes_to_existing_query_runtime() {
    let app = app(
        "query = { code: \"synthetic-code\", state: \"synthetic-state\" }.to_query\n    render plain: query",
    );
    let output = source(&app);
    assert!(
        output.contains("ActionView::ViewHelpers.hash_to_query({ code:"),
        "{output}"
    );
    assert!(!output.contains("}.to_query"));
    let errors: Vec<_> = roundhouse::analyze::diagnose(&app)
        .into_iter()
        .filter(|d| d.severity == roundhouse::diagnostic::Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn unproved_targets_reject_lowered_query_calls() {
    let app = app("render plain: { b: \"2\", a: \"1\" }.to_query");
    for target in BuildTarget::TRANSPILE {
        if matches!(target, BuildTarget::Ruby | BuildTarget::Spinel) {
            continue;
        }
        let error = roundhouse::project::target_files(&app, Path::new("."), *target).unwrap_err();
        assert!(
            error.contains("Hash#to_query requires the verified Ruby or Spinel query grammar"),
            "{target:?}: {error}"
        );
    }
}

#[test]
fn unsupported_arity_keeps_error_and_does_not_lower() {
    let app = app("render plain: { a: \"1\" }.to_query(\"one\", \"two\")");
    assert!(!source(&app).contains("hash_to_query"));
    assert!(
        roundhouse::analyze::diagnose(&app)
            .iter()
            .any(|d| d.severity == roundhouse::diagnostic::Severity::Error)
    );
}

#[test]
fn unproved_domains_blocks_and_namespaces_keep_diagnostics() {
    for body in [
        "render plain: { a: Object.new }.to_query",
        "render plain: { Object.new => \"x\" }.to_query",
        "render plain: { nil => \"x\" }.to_query",
        "render plain: { a: QueryRecord.find(1).payload }.to_query",
        "render plain: { a: QueryRecord.find(1) }.to_query",
        "render plain: { a: \"x\" }.to_query { 1 }",
        "render plain: { a: \"x\" }.to_query(42)",
        "namespace = params[:selector] ? \"return\" : 42\n    render plain: { a: \"x\" }.to_query(namespace)",
    ] {
        let app = app(body);
        assert!(!source(&app).contains("hash_to_query"), "{body}");
        assert!(
            roundhouse::analyze::diagnose(&app)
                .iter()
                .any(|d| d.severity == roundhouse::diagnostic::Severity::Error
                    && d.message.contains("to_query")),
            "{body}"
        );
    }
}

fn default_app(controller: &str, view: &str) -> roundhouse::App {
    let tree = [
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/query\" => \"queries#index\"\nend\n",
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("app/controllers/queries_controller.rb", controller),
        ("app/views/queries/_query.html.erb", view),
    ]
    .into_iter()
    .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).unwrap();
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn assert_unproved_default_is_rejected(app: roundhouse::App) {
    let raw = ruby::emit_spinel(&app);
    assert!(
        raw.iter().any(|file| file.content.contains("}.to_query")),
        "expected the actual default query call to survive ingestion"
    );
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let result = roundhouse::project::target_files(&app, Path::new("."), target);
        assert!(
            result.is_err(),
            "{target:?} emitted an unproved query default"
        );
        let error = result.err().unwrap();
        assert!(error.contains("unlowered Hash#to_query"), "{error}");
    }
}

#[test]
fn untyped_controller_keyword_query_default_is_rejected() {
    assert_unproved_default_is_rejected(default_app(
        "class QueriesController < ApplicationController\n  def index\n    render plain: label\n  end\n  private\n  def label(query: { a: \"x\" }.to_query)\n    query\n  end\nend\n",
        "query\n",
    ));
}

#[test]
fn untyped_view_strict_local_query_default_is_rejected() {
    let template = "<%# locals: (record:, query: { a: \"x\" }.to_query) -%>\n<%= query %>\n";
    let result = roundhouse::ingest::ingest_view(
        template,
        Path::new("queries/_query.html.erb"),
        "app/views/queries/_query.html.erb",
    );
    assert!(result.is_err());
    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("strict-local to_query defaults are not supported")
    );
}

#[test]
fn strict_local_query_defaults_reject_ruby_literal_boundaries() {
    for default in [
        r#"{ a: ")" }.to_query"#,
        r#"{ a: '(' }.to_query"#,
        r#"{ a: "\"), (" }.to_query"#,
        r#"{ a: '\'), (' }.to_query"#,
        r#"{ a: %q{)} }.to_query"#,
        r#"{ a: %q|(,)| }.to_query"#,
        r#"{ a: %Q[(#{1})] }.to_query"#,
        r#"{ a: /\)/ }.to_query"#,
        r#"{ a: %r{\)} }.to_query"#,
        r#"{ a: /[(]/ }.to_query"#,
    ] {
        let template = format!("<%# locals: (record:, query: {default}) -%>\n<%= query %>\n");
        let result = roundhouse::ingest::ingest_view(
            &template,
            Path::new("queries/_query.html.erb"),
            "app/views/queries/_query.html.erb",
        );
        assert!(
            result.is_err(),
            "silently accepted query default: {default}"
        );
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("strict-local to_query defaults are not supported"),
            "{default}"
        );
    }
}

#[test]
fn strict_local_literal_query_text_does_not_trigger_call_rejection() {
    for default in [r#"")to_query""#, "')to_query'", r#""\"),to_query(""#] {
        let template = format!("<%# locals: (record:, query: {default}) -%>\n<%= query %>\n");
        let view = roundhouse::ingest::ingest_view(
            &template,
            Path::new("queries/_query.html.erb"),
            "app/views/queries/_query.html.erb",
        )
        .unwrap();
        let locals = view.strict_locals.unwrap();
        assert_eq!(locals.len(), 2);
        assert!(
            matches!(
                &*locals[1].default.as_ref().unwrap().node,
                roundhouse::expr::ExprNode::Lit {
                    value: roundhouse::expr::Literal::Str { .. }
                }
            ),
            "{default}"
        );
    }
}

#[test]
fn strict_local_signature_body_is_rejected() {
    let template = "<%# locals: (record:, query: \"x\"); { a: \"x\" }.to_query() -%>\n";
    let result = roundhouse::ingest::ingest_view(
        template,
        Path::new("queries/_query.html.erb"),
        "app/views/queries/_query.html.erb",
    );
    assert!(result.is_err());
    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("strict-local signature contains body statements")
    );
}

#[test]
fn strict_local_positional_optional_parameters_are_rejected() {
    for default in ["query = { a: \"x\" }.to_query", "query = 42"] {
        let template = format!("<%# locals: ({default}, record:) -%>\n<p>synthetic</p>\n");
        let result = roundhouse::ingest::ingest_view(
            &template,
            Path::new("queries/_query.html.erb"),
            "app/views/queries/_query.html.erb",
        );
        assert!(
            result.is_err(),
            "silently omitted positional default: {default}"
        );
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("strict-local positional optional parameters are not supported")
        );
    }
}

#[test]
fn strict_local_header_discovery_is_not_masked_by_ordinary_text() {
    for prefix in [
        "<div>locals:</div>\n",
        "<%# unrelated locals: mention %>\n",
        "<!-- locals: -->\n",
        "<p># locals: not a signature</p>\n",
    ] {
        let template =
            format!("{prefix}<%# locals: (query: {{ a: \"x\" }}.to_query) -%>\n<p>synthetic</p>\n");
        let result = roundhouse::ingest::ingest_view(
            &template,
            Path::new("queries/_query.html.erb"),
            "app/views/queries/_query.html.erb",
        );
        assert!(
            result.is_err(),
            "ordinary text masked real strict-local header: {prefix}"
        );
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("strict-local to_query defaults are not supported")
        );
    }
}
