//! A shared STI form must submit to the record's subtype, not its base.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn shared_form_uses_sti_member_and_collection_routes() {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base; end",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base; primary_abstract_class; end",
        )
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table :rooms do |t|
    t.string :name
    t.string :type
  end
end
"#,
        )
        .write("app/models/room.rb", "class Room < ApplicationRecord; end")
        .write("app/models/rooms/open.rb", "class Rooms::Open < Room; end")
        .write("app/models/rooms/closed.rb", "class Rooms::Closed < Room; end")
        .write(
            "config/routes.rb",
            r#"Rails.application.routes.draw do
  resources :rooms
  namespace :rooms do
    resources :opens
    resources :closeds
  end
end
"#,
        )
        .write(
            "app/views/rooms/_form.html.erb",
            "<%= form_with model: room do |form| %><%= form.text_field :name %><% end %>",
        )
        .write(
            "app/views/rooms/_explicit.html.erb",
            "<%= form_with model: room, url: '/custom' do |form| %><% end %>",
        )
        .write(
            "app/views/rooms/_record_url.html.erb",
            "<%= form_with url: room do |form| %><% end %>",
        )
        .run_ruby(r#"
[['Rooms::Open', 'opens'], ['Rooms::Closed', 'closeds']].each do |type, route|
  room = Room.new(name: 'Lounge', type: type)
  html = Views::Rooms.form(room)
  raise "wrong collection route: #{html}" unless html.include?(%(action="/rooms/#{route}"))
  raise 'record URL missed subtype' unless Views::Rooms.record_url(room).include?(%(action="/rooms/#{route}"))
  raise "new record must POST: #{html}" if html.include?('name="_method"')
  room.save!
  html = Views::Rooms.form(room)
  raise "wrong member route: #{html}" unless html.include?(%(action="/rooms/#{route}/#{room.id}"))
  raise 'record URL missed subtype' unless Views::Rooms.record_url(room).include?(%(action="/rooms/#{route}/#{room.id}"))
  raise "saved record must PATCH: #{html}" unless html.include?('value="patch"')
  html = Views::Rooms.explicit(room)
  raise "explicit URL must win: #{html}" unless html.include?('action="/custom"')
end
room = Room.new(name: 'Base room')
raise 'base route changed' unless Views::Rooms.form(room).include?('action="/rooms"')
puts 'STI form routes passed'
"#)
        .assert_passes();
}
