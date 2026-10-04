use roundhouse::diagnostic::Severity;
use roundhouse::ingest::ingest_app_from_tree;
use std::path::PathBuf;

const RECORD_SERVICE: &str = r#"class SourceYieldProbe
  def self.record(id, allowed, produce)
    raise "denied" unless allowed
    return nil unless produce
    yield YieldRecord.find(id)
  end
  def self.tuple(id, allowed, produce)
    record(id, allowed, produce) do |record|
      yield record, "window", 7
    end
  end
end
"#;

fn app(action: &str) -> roundhouse::App {
    app_with_service(action, RECORD_SERVICE)
}

fn app_with_service(action: &str, service: &str) -> roundhouse::App {
    app_with_signature(action, service, None)
}

fn app_with_signature(action: &str, service: &str, signature: Option<&str>) -> roundhouse::App {
    let mut files: Vec<_> = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"yield_records\" do |t|\n    t.string \"title\", null: false\n    t.jsonb \"payload\"\n  end\nend\n".to_string()),
        ("app/models/yield_record.rb", "class YieldRecord < ApplicationRecord\nend\n".to_string()),
        ("app/services/source_yield_probe.rb", service.to_string()),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/yield\" => \"yields#index\"\nend\n".to_string()),
        ("app/controllers/yields_controller.rb", format!("class YieldsController < ApplicationController\n  def index\n    {action}\n  end\nend\n")),
    ].into_iter().map(|(path, source)| (PathBuf::from(path), source.into_bytes())).collect();
    if let Some(signature) = signature {
        files.push((
            PathBuf::from("sig/source_yield_probe.rbs"),
            signature.as_bytes().to_vec(),
        ));
    }
    let mut app = ingest_app_from_tree(files.into_iter().collect()).unwrap();
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

#[test]
fn source_record_yield_types_controller_block() {
    let app = app(
        "SourceYieldProbe.record(1, true, true) { |record| @record = record }\n    render plain: @record.title",
    );
    let errors: Vec<_> = roundhouse::analyze::diagnose(&app)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn nested_source_tuple_yield_keeps_each_position() {
    let app = app(
        "SourceYieldProbe.tuple(1, true, true) { |record, label, count| @record, @label, @count = record, label, count }\n    if @record && @label && @count\n      render plain: @record.title + @label.to_s + @count.to_s\n    else\n      render plain: \"empty\"\n    end",
    );
    let errors: Vec<_> = roundhouse::analyze::diagnose(&app)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn zero_yield_preserves_nil_seed_and_guarded_read() {
    let app = app(
        "@record = nil\n    SourceYieldProbe.record(1, true, false) { |record| @record = record }\n    render plain: @record ? @record.title : \"empty\"",
    );
    let errors: Vec<_> = roundhouse::analyze::diagnose(&app)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn unguarded_nullable_tuple_value_keeps_error() {
    let app = app(
        "@label = nil\n    SourceYieldProbe.tuple(1, true, false) { |record, label, count| @label = label }\n    render plain: \"prefix\" + @label",
    );
    assert!(
        roundhouse::analyze::diagnose(&app)
            .iter()
            .any(|d| d.severity == Severity::Error && d.message.contains("String?"))
    );
}

#[test]
fn opaque_array_and_splat_yields_keep_errors() {
    for value in [
        "YieldRecord.find(1).payload",
        "[YieldRecord.find(1), \"label\"]",
        "*[YieldRecord.find(1), \"label\"]",
    ] {
        let service = format!(
            "class SourceYieldProbe\n  def self.record(id, allowed, produce)\n    yield {value}\n  end\nend\n"
        );
        let app = app_with_service(
            "SourceYieldProbe.record(1, true, true) { |record, label| @record, @label = record, label }\n    render plain: @record.title + @label",
            &service,
        );
        assert!(
            roundhouse::analyze::diagnose(&app)
                .iter()
                .any(|d| d.severity == Severity::Error),
            "{value}"
        );
    }
}

#[test]
fn ambiguous_class_and_instance_yields_keep_errors() {
    let service = "class SourceYieldProbe\n  def self.record(id, allowed, produce)\n    yield YieldRecord.find(id)\n  end\n  def record(id, allowed, produce)\n    yield \"instance\"\n  end\nend\n";
    let app = app_with_service(
        "SourceYieldProbe.record(1, true, true) { |record| @record = record }\n    render plain: @record.title",
        service,
    );
    assert!(
        roundhouse::analyze::diagnose(&app)
            .iter()
            .any(|d| d.severity == Severity::Error)
    );
}

#[test]
fn splatted_multiassign_retains_unproved_position_error() {
    let app =
        app("@record, @label = *[YieldRecord.find(1), \"label\"]\n    render plain: @record.title + @label");
    assert!(
        roundhouse::analyze::diagnose(&app)
            .iter()
            .any(|d| d.severity == Severity::Error)
    );
}

#[test]
fn explicit_rbs_block_is_not_replaced_by_source_yield() {
    let app = app_with_signature(
        "SourceYieldProbe.record(1, true, true) { |record| @record = record }\n    render plain: @record.title",
        RECORD_SERVICE,
        Some(
            "class SourceYieldProbe\n  def self.record: (Integer, bool, bool) { (String) -> void } -> void\nend\n",
        ),
    );
    assert!(
        roundhouse::analyze::diagnose(&app)
            .iter()
            .any(|d| d.severity == Severity::Error)
    );
}

#[test]
fn nested_yield_signature_keeps_record_string_integer_slots() {
    let app = app(
        "SourceYieldProbe.tuple(1, true, true) { |record, label, count| @record, @label, @count = record, label, count }\n    render plain: @label.to_s + @count.to_s",
    );
    let service = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "SourceYieldProbe")
        .unwrap();
    let method = service
        .methods
        .iter()
        .find(|m| m.name.as_str() == "tuple")
        .unwrap();
    let roundhouse::ty::Ty::Fn {
        block: Some(block), ..
    } = method.signature.as_ref().unwrap()
    else {
        panic!("missing block signature")
    };
    let roundhouse::ty::Ty::Fn { params, .. } = &**block else {
        panic!("missing positional yield signature")
    };
    assert_eq!(params.len(), 3);
    assert!(matches!(&params[0].ty, roundhouse::ty::Ty::Class { id, .. } if id.0.as_str() == "YieldRecord"));
    assert_eq!(params[1].ty, roundhouse::ty::Ty::Str);
    assert_eq!(params[2].ty, roundhouse::ty::Ty::Int);
}

#[test]
fn compound_guard_narrowing_remains_unproved() {
    let app = app(
        "@label = nil\n    SourceYieldProbe.tuple(1, true, true) { |record, label, count| @label = label }\n    if @record && @label && @count\n      render plain: \"prefix\" + @label\n    end",
    );
    assert!(
        roundhouse::analyze::diagnose(&app)
            .iter()
            .any(|d| d.severity == Severity::Error)
    );
}

#[test]
fn class_objects_self_and_opaque_parameter_origins_keep_errors() {
    for body in [
        "yield YieldRecord",
        "row = YieldRecord\n    yield row",
        "yield self",
        "row = self\n    yield row",
        "yield id",
        "row = id\n    yield row",
    ] {
        let service = format!(
            "class SourceYieldProbe\n  def self.record(id, allowed, produce)\n    {body}\n  end\n  def title\n    \"instance title\"\n  end\nend\n"
        );
        let app = app_with_service(
            "SourceYieldProbe.record(YieldRecord, true, true) { |record| @record = record }\n    render plain: @record.title",
            &service,
        );
        assert!(
            roundhouse::analyze::diagnose(&app)
                .iter()
                .any(|d| d.severity == Severity::Error),
            "{body}"
        );
    }
}

#[test]
fn lookup_spelling_does_not_prove_record_instance_origin() {
    for value in [
        "LookupTrap.find(1)",
        "LookupTrap.new",
        "[YieldRecord].first",
        "row = LookupTrap.find(1)\n    copy = row\n    copy",
        "row = [YieldRecord].first\n    copy = row\n    copy",
    ] {
        let body = if value.contains('\n') {
            let (setup, last) = value.rsplit_once('\n').unwrap();
            format!("{setup}\n    yield {last}")
        } else {
            format!("yield {value}")
        };
        let service = format!(
            "class LookupTrap\n  def self.find(id)\n    YieldRecord\n  end\n  def self.new\n    YieldRecord\n  end\nend\nclass SourceYieldProbe\n  def self.record(id, allowed, produce)\n    {body}\n  end\nend\n"
        );
        let app = app_with_service(
            "SourceYieldProbe.record(1, true, true) { |record| @record = record }\n    render plain: @record ? @record.title : \"empty\"",
            &service,
        );
        assert!(
            roundhouse::analyze::diagnose(&app)
                .iter()
                .any(|d| d.severity == Severity::Error),
            "{value}"
        );
    }
}

#[test]
fn class_object_array_iterator_origins_keep_errors() {
    for body in [
        "[YieldRecord].each { |record| yield record }",
        "rows = [YieldRecord]\n    rows.each { |record| yield record }",
        "rows = [YieldRecord]\n    copy = rows\n    copy.each { |record| yield record }",
        "record, label = YieldRecord, \"class\"\n    yield record",
    ] {
        let service = format!(
            "class SourceYieldProbe\n  def self.record(id, allowed, produce)\n    {body}\n  end\nend\n"
        );
        let app = app_with_service(
            "SourceYieldProbe.record(1, true, true) { |record| @record = record }\n    render plain: @record ? @record.title : \"empty\"",
            &service,
        );
        assert!(
            roundhouse::analyze::diagnose(&app)
                .iter()
                .any(|d| d.severity == Severity::Error),
            "{body}"
        );
    }
}

#[test]
fn array_and_multiple_id_finders_are_not_single_record_yields() {
    for body in [
        "yield YieldRecord.find([1])",
        "row = YieldRecord.find([1])\n    yield row",
        "yield YieldRecord.find(1, 2)",
        "row = YieldRecord.find(1, 2)\n    yield row",
        "yield YieldRecord.find(1) { |record| true }",
        "row = YieldRecord.find(1) { |record| true }\n    yield row",
        "yield YieldRecord.find(nil)",
        "yield YieldRecord.find(\"1\")",
    ] {
        let service = format!(
            "class SourceYieldProbe\n  def self.record(id, allowed, produce)\n    {body}\n  end\nend\n"
        );
        let app = app_with_service(
            "SourceYieldProbe.record(1, true, true) { |record| @record = record }\n    render plain: @record ? @record.title : \"empty\"",
            &service,
        );
        assert!(
            roundhouse::analyze::diagnose(&app)
                .iter()
                .any(|d| d.severity == Severity::Error),
            "{body}"
        );
    }
}

#[test]
fn class_origin_operator_reassignment_keeps_error() {
    let service = "class SourceYieldProbe\n  def self.record(id, allowed, produce)\n    row = YieldRecord.find(1)\n    row &&= YieldRecord\n    yield row\n  end\nend\n";
    let app = app_with_service(
        "SourceYieldProbe.record(1, true, true) { |record| @record = record }\n    render plain: @record ? @record.title : \"empty\"",
        service,
    );
    assert!(
        roundhouse::analyze::diagnose(&app)
            .iter()
            .any(|d| d.severity == Severity::Error)
    );
}

#[test]
fn inherited_instance_dispatch_does_not_use_class_yield_contract() {
    let service = "class YieldParent\n  def record(id, allowed, produce)\n    yield \"parent value\"\n  end\nend\nclass SourceYieldProbe < YieldParent\n  def self.record(id, allowed, produce)\n    yield YieldRecord.find(id)\n  end\nend\n";
    let app = app_with_service(
        "SourceYieldProbe.new.record(1, true, true) { |record| @record = record }\n    render plain: @record ? @record.title : \"empty\"",
        service,
    );
    assert!(
        roundhouse::analyze::diagnose(&app)
            .iter()
            .any(|d| d.severity == Severity::Error)
    );
}
