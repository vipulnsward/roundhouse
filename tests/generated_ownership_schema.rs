use roundhouse::emit::shared::schema_sql::{Dialect, render_schema_statements_for};
use roundhouse::ingest::schema::ingest_schema;
use std::process::Command;

const SOURCE: &str = r#"
ActiveRecord::Schema.define do
  create_table "orders" do |t|
    t.bigint "event_id"
    t.virtual "ownership_key", type: :bigint, as: "COALESCE(event_id, (0)::bigint)", stored: true
    t.index ["id", "ownership_key"], unique: true, name: "orders_ownership"
    t.check_constraint "event_id IS NULL OR event_id > 0", name: "orders_positive_event"
  end
  create_table "ticket_types" do |t|
    t.bigint "event_id"
    t.virtual "ownership_key", type: :bigint, as: "COALESCE(event_id, (0)::bigint)", stored: true
    t.index ["id", "ownership_key"], unique: true, name: "types_ownership"
  end
  create_table "tickets" do |t|
    t.bigint "order_id", null: false
    t.bigint "ticket_type_id", null: false
    t.bigint "event_id"
    t.integer "price_paise", null: false
    t.virtual "ownership_key", type: :bigint, as: "COALESCE(event_id, (0)::bigint)", stored: true
    t.check_constraint "event_id IS NULL OR event_id > 0", name: "tickets_positive_event"
    t.check_constraint "event_id IS NULL OR price_paise = 0", name: "owned_tickets_free_only"
  end
  add_foreign_key "tickets", "orders", column: ["order_id", "ownership_key"], primary_key: ["id", "ownership_key"]
  add_foreign_key "tickets", "ticket_types", column: ["ticket_type_id", "ownership_key"], primary_key: ["id", "ownership_key"]
end
"#;

#[test]
fn generated_tenant_schema_retains_and_renders_constraints() {
    let schema = ingest_schema(SOURCE.as_bytes(), "ownership/schema.rb").expect("generated columns are supported without omission");
    let sql = render_schema_statements_for(&schema, Dialect::Sqlite).expect("portable ownership constraints").join(";\n");
    assert!(sql.contains("GENERATED ALWAYS AS"));
    assert!(sql.contains("FOREIGN KEY (order_id, ownership_key)"));
    assert!(sql.contains("FOREIGN KEY (ticket_type_id, ownership_key)"));
    assert!(sql.contains("CHECK"));
}

#[test]
#[ignore = "requires Python 3 with SQLite 3.31+; run explicitly for SQLite oracle validation"]
fn generated_tenant_schema_enforces_insert_update_constraints() {
    let schema = ingest_schema(SOURCE.as_bytes(), "ownership/schema.rb").unwrap();
    let sql = render_schema_statements_for(&schema, Dialect::Sqlite).unwrap().join(";\n");
    let retained = std::env::var_os("DQOR_SCHEMA_EVIDENCE");
    let out = retained.as_ref().map(std::path::PathBuf::from).unwrap_or_else(|| {
        std::env::temp_dir().join(format!("roundhouse-ownership-{}", std::process::id()))
    });
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("generated-schema.sql"), &sql).unwrap();
    let script = r#"
import sqlite3,sys
c=sqlite3.connect(':memory:')
c.execute('PRAGMA foreign_keys=ON')
c.executescript(open(sys.argv[1]).read())
def reject(sql):
    try: c.execute(sql)
    except sqlite3.IntegrityError: return
    raise AssertionError('accepted invalid write: '+sql)
c.executescript('INSERT INTO orders(id,event_id) VALUES(1,NULL),(2,7),(3,8);INSERT INTO ticket_types(id,event_id) VALUES(1,NULL),(2,7),(3,8);INSERT INTO tickets(id,order_id,ticket_type_id,event_id,price_paise) VALUES(1,1,1,NULL,100),(2,2,2,7,0);')
assert c.execute('SELECT ownership_key FROM orders ORDER BY id').fetchall()==[(0,),(7,),(8,)]
assert c.execute('SELECT ownership_key FROM tickets ORDER BY id').fetchall()==[(0,),(7,)]
reject('INSERT INTO tickets(id,order_id,ticket_type_id,event_id,price_paise) VALUES(3,2,3,7,0)')
reject('INSERT INTO tickets(id,order_id,ticket_type_id,event_id,price_paise) VALUES(3,2,2,NULL,0)')
reject('UPDATE tickets SET ticket_type_id=3 WHERE id=2')
reject('UPDATE tickets SET event_id=8 WHERE id=2')
reject('UPDATE orders SET event_id=8 WHERE id=2')
reject('UPDATE ticket_types SET event_id=8 WHERE id=2')
reject('UPDATE tickets SET price_paise=10 WHERE id=2')
reject('UPDATE tickets SET price_paise=2147483648 WHERE id=1')
reject('INSERT INTO orders(id,event_id) VALUES(4,-1)')
reject("INSERT INTO orders(id,event_id) VALUES(4,'foreign')")
reject("UPDATE orders SET event_id='foreign' WHERE id=3")
reject("INSERT INTO ticket_types(id,event_id) VALUES(4,'foreign')")
reject("UPDATE tickets SET event_id='foreign' WHERE id=2")
try: c.execute('UPDATE tickets SET ownership_key=8 WHERE id=2')
except sqlite3.OperationalError as e: assert 'generated column' in str(e)
else: raise AssertionError('generated key was writable')
c.execute('INSERT INTO orders(id,event_id) VALUES(4,NULL)')
c.execute('UPDATE orders SET event_id=9 WHERE id=4')
assert c.execute('SELECT ownership_key FROM orders WHERE id=4').fetchone()==(9,)
assert c.execute('SELECT id,order_id,ticket_type_id,event_id,ownership_key,price_paise FROM tickets ORDER BY id').fetchall()==[(1,1,1,None,0,100),(2,2,2,7,7,0)]
print('ownership schema insert/update enforcement passed')
"#;
    let result = Command::new("python3").args(["-c", script]).arg(out.join("generated-schema.sql")).output().unwrap();
    if retained.is_none() { std::fs::remove_dir_all(&out).unwrap(); }
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    print!("{}", String::from_utf8_lossy(&result.stdout));
}

#[test]
fn unsupported_check_expression_is_rejected_instead_of_omitted() {
    let source = r#"ActiveRecord::Schema.define do
      create_table "events" do |t|
        t.string "status"
        t.check_constraint "status::text = ANY (ARRAY['draft'::text, 'published'::text])", name: "status_guard"
      end
    end"#;
    let schema = ingest_schema(source.as_bytes(), "checks/schema.rb").unwrap();
    let error = render_schema_statements_for(&schema, Dialect::Sqlite).unwrap_err();
    assert!(error.contains("check constraint"));
    assert!(render_schema_statements_for(&schema, Dialect::Postgres).unwrap().join("\n").contains("status::text"));
}

#[test]
fn malformed_composite_foreign_key_fails_ingestion() {
    let source = r#"ActiveRecord::Schema.define do
      create_table "orders" do |t| t.bigint "event_id" end
      create_table "tickets" do |t| t.bigint "order_id"; t.bigint "event_id" end
      add_foreign_key "tickets", "orders", column: ["order_id", "event_id"], primary_key: ["id"]
    end"#;
    let error = ingest_schema(source.as_bytes(), "invalid/schema.rb").unwrap_err().to_string();
    assert!(error.contains("unequal column arity"));
}

#[test]
fn unsupported_unique_index_does_not_widen_uniqueness() {
    let source = r#"ActiveRecord::Schema.define do
      create_table "tokens" do |t|
        t.bigint "user_id"
        t.string "kind"
        t.index ["user_id"], unique: true, name: "initial_tokens", where: "((kind)::text = 'initial'::text)"
      end
    end"#;
    let schema = ingest_schema(source.as_bytes(), "indexes/schema.rb").unwrap();
    assert!(render_schema_statements_for(&schema, Dialect::Sqlite).unwrap_err().contains("initial_tokens"));
    assert!(render_schema_statements_for(&schema, Dialect::Postgres).unwrap()[1].contains("WHERE ((kind)::text"));
}

#[test]
fn unsupported_foreign_key_options_and_actions_fail_closed() {
    for option in ["deferrable: :deferred", "validate: false", "match: :full", "on_delete: :unknown", "on_update: action"] {
        let source = format!(r#"ActiveRecord::Schema.define do
          create_table "orders" do |t| t.bigint "event_id" end
          create_table "tickets" do |t| t.bigint "order_id" end
          add_foreign_key "tickets", "orders", {option}
        end"#);
        assert!(ingest_schema(source.as_bytes(), "options/schema.rb").is_err(), "{option}");
    }
}

#[test]
fn foreign_key_declarations_cannot_disappear() {
    for declaration in [
        "add_foreign_key from_table, 'orders'",
        "add_foreign_key 'tickets', to_table",
        "add_foreign_key 'tickets', 'orders', options",
        "add_foreign_key 'missing', 'orders'",
        "add_foreign_key 'tickets', 'missing'",
    ] {
        let source = format!(r#"ActiveRecord::Schema.define do
          create_table "orders" do |t| t.bigint "event_id" end
          create_table "tickets" do |t| t.bigint "order_id" end
          {declaration}
        end"#);
        assert!(ingest_schema(source.as_bytes(), "missing/schema.rb").is_err(), "{declaration}");
    }
}

#[test]
fn check_options_cannot_disappear() {
    for option in ["validate: false", "**options", "name: name", "'name'"] {
        let source = format!(r#"ActiveRecord::Schema.define do
          create_table "orders" do |t|
            t.bigint "event_id"
            t.check_constraint "event_id > 0", {option}
          end
        end"#);
        assert!(ingest_schema(source.as_bytes(), "check-options/schema.rb").is_err(), "{option}");
    }
}

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn emitted_orm_preserves_generated_ownership_writes_and_reload() {
    emit_and_run::empty_app()
        .write("db/schema.rb", SOURCE)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n")
        .write("app/models/order.rb", "class Order < ApplicationRecord\nend\n")
        .write("app/models/ticket_type.rb", "class TicketType < ApplicationRecord\nend\n")
        .write("app/models/ticket.rb", "class Ticket < ApplicationRecord\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .run_ruby(r#"
legacy_order = Order.create!(event_id: nil)
tenant_order = Order.create!(event_id: 7)
other_order = Order.create!(event_id: 8)
legacy_type = TicketType.create!(event_id: nil)
tenant_type = TicketType.create!(event_id: 7)
other_type = TicketType.create!(event_id: 8)
legacy = Ticket.create!(order_id: legacy_order.id, ticket_type_id: legacy_type.id, event_id: nil, price_paise: 100)
tenant = Ticket.create!(order_id: tenant_order.id, ticket_type_id: tenant_type.id, event_id: 7, price_paise: 0)
legacy.reload
tenant.reload
raise "legacy key" unless legacy.ownership_key == 0
raise "tenant key" unless tenant.ownership_key == 7
begin
  tenant.update!(ticket_type_id: other_type.id)
  raise "accepted cross-owner update"
rescue StandardError => error
  raise error unless error.message.include?("FOREIGN KEY constraint failed")
end
tenant.reload
raise "invalid update persisted" unless tenant.ticket_type_id == tenant_type.id
begin
  Ticket.create!(order_id: other_order.id, ticket_type_id: tenant_type.id, event_id: 7, price_paise: 0)
  raise "accepted cross-owner create"
rescue StandardError => error
  raise error unless error.message.include?("FOREIGN KEY constraint failed")
end
free_order = Order.create!(event_id: nil)
free_order.update!(event_id: 9)
free_order.reload
raise "updated generated key" unless free_order.ownership_key == 9
raise "extra ticket inserted" unless Ticket.count == 2
puts "emitted ownership ORM passed"
"#).assert_passes();
}

#[test]
fn generated_options_are_literal_and_supported_or_fail_closed() {
    for options in [
        "as: 'COALESCE(event_id, 0)', stored: true",
        "type: type, as: 'COALESCE(event_id, 0)', stored: true",
        "type: :bigint, as: expression, stored: true",
        "type: :bigint, as: 'COALESCE(event_id, 0)'",
        "type: :bigint, as: 'COALESCE(event_id, 0)', stored: mode",
        "type: :bigint, as: 'COALESCE(event_id, 0)', stored: true, default: 0",
        "type: :bigint, as: 'COALESCE(event_id, 0)', stored: true, default: value",
        "type: :bigint, as: 'COALESCE(event_id, 0)', stored: true, **options",
        "type: :bigint, as: 'COALESCE(event_id, 0)', stored: true, null: nullable",
        "type: :string, as: 'COALESCE(event_id, 0)', stored: true",
    ] {
        let source = format!(r#"ActiveRecord::Schema.define do
          create_table "orders" do |t|
            t.bigint "event_id"
            t.virtual "ownership_key", {options}
          end
        end"#);
        assert!(ingest_schema(source.as_bytes(), "generated-options/schema.rb").is_err(), "{options}");
    }
    let source = r#"ActiveRecord::Schema.define do
      create_table "orders" do |t|
        t.bigint "event_id"
        t.virtual "ownership_key", type: :bigint, as: "abs(event_id)", stored: true
      end
    end"#;
    let schema = ingest_schema(source.as_bytes(), "expression/schema.rb").unwrap();
    assert!(render_schema_statements_for(&schema, Dialect::Sqlite).is_err());
}

#[test]
fn generated_constraints_require_materialized_migrations() {
    use roundhouse::ingest::schema::ingest_migration;
    for mutation in ["rename_column :orders, :event_id, :owner", "remove_column :orders, :event_id", "change_column :orders, :event_id, :string", "rename_table :orders, :purchases", "remove_foreign_key :tickets, :orders", "add_check_constraint :orders, 'event_id > 0'"] {
        let mut schema = ingest_schema(SOURCE.as_bytes(), "ownership/schema.rb").unwrap();
        let migration = format!("class ChangeOwnership < ActiveRecord::Migration[8.1]\n  def change\n    {mutation}\n  end\nend\n");
        assert!(ingest_migration(migration.as_bytes(), "mutation.rb", &mut schema).is_err(), "{mutation}");
    }
}

#[test]
fn schema_constraint_metadata_round_trips_and_old_json_defaults() {
    let schema = ingest_schema(SOURCE.as_bytes(), "ownership/schema.rb").unwrap();
    let encoded = serde_json::to_value(&schema).unwrap();
    assert_eq!(schema, serde_json::from_value(encoded.clone()).unwrap());
    let mut old = encoded;
    for table in old["tables"].as_object_mut().unwrap().values_mut() {
        table.as_object_mut().unwrap().remove("constraints");
    }
    let previous: roundhouse::Schema = serde_json::from_value(old).unwrap();
    assert!(previous.tables.values().all(|table| table.constraints == Default::default()));
}

#[test]
fn unsupported_target_and_roda_seed_predicate_return_errors() {
    use roundhouse::project::{BuildTarget, target_files};
    let tree = |source: &str| {
        std::collections::HashMap::from([
            (std::path::PathBuf::from("db/schema.rb"), source.as_bytes().to_vec()),
            (std::path::PathBuf::from("config/routes.rb"), b"Rails.application.routes.draw do\nend\n".to_vec()),
        ])
    };
    let app = roundhouse::ingest::ingest_app_from_tree(tree(SOURCE)).unwrap();
    for target in [BuildTarget::Roda, BuildTarget::Jruby, BuildTarget::Go] {
        assert!(target_files(&app, std::path::Path::new("."), target).unwrap_err().contains("constraints"));
    }
    let source = r#"ActiveRecord::Schema.define do
      create_table "tokens" do |t|
        t.bigint "user_id"
        t.string "kind"
        t.index ["user_id"], unique: true, name: "initial_tokens", where: "((kind)::text = 'initial'::text)"
      end
    end"#;
    let app = roundhouse::ingest::ingest_app_from_tree(tree(source)).unwrap();
    assert!(target_files(&app, std::path::Path::new("."), BuildTarget::Roda).unwrap_err().contains("initial_tokens"));
}

#[test]
fn arithmetic_check_semantics_are_not_assumed_portable() {
    for expression in ["amount / divisor > 0", "amount % divisor > 0", "amount + divisor > 0", "amount * divisor > 0", "amount - divisor > 0", "abs(amount) > 0"] {
        let source = format!(r#"ActiveRecord::Schema.define do
          create_table "orders" do |t|
            t.bigint "amount"
            t.bigint "divisor"
            t.check_constraint "{expression}"
          end
        end"#);
        let schema = ingest_schema(source.as_bytes(), "arithmetic/schema.rb").unwrap();
        assert!(render_schema_statements_for(&schema, Dialect::Sqlite).is_err(), "{expression}");
        assert!(render_schema_statements_for(&schema, Dialect::Postgres).unwrap()[0].contains(expression));
    }
}
