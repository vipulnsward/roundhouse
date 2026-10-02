#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn button_form_options_and_parameters_survive_lowering_and_emission() {
    emit_and_run::real_blog()
        .write(
            "app/views/articles/_google.html.erb",
            r#"<%= button_to "Google", "/session/google", form: { data: { turbo: false } }, params: { join_code: article.title } %>
<%= button_to "/session/google", form: { data: { turbo: false } }, params: { join_code: article.title } do %>Google block<% end %>"#,
        )
        .run_ruby(r#"
html = Views::Articles.google(Article.new(title: "invite<&")) + ActionView::ViewHelpers.button_to("Google runtime", "/session/google", { form: { data: { turbo: false } }, params: { join_code: "invite<&" } })
raise "expected three forms" unless html.scan('<form ').length == 3
raise "form must own turbo opt-out" unless html.scan('data-turbo="false"').length == 3
html.scan(/<form\b[^>]*>/).each do |form|
  raise "wrong form action or method" unless form.include?('action="/session/google"') && form.include?('method="post"') && form.include?('data-turbo="false"')
end
html.scan(/<button\b[^>]*>/).each do |button|
  raise "form options leaked to button" if button.include?('form=') || button.include?('params=') || button.include?('data-turbo=')
end
raise "missing escaped join parameter" unless html.scan('name="join_code" value="invite&lt;&amp;"').length == 3
raise "missing csrf fields" unless html.scan('name="authenticity_token"').length == 3
"#)
        .assert_passes();
}
