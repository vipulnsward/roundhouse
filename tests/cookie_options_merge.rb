require_relative "../runtime/ruby/test/test_helper"
require_relative "../runtime/spinel/cgi_io"
require "stringio"

overlay_path = File.expand_path("../runtime/spinel/scaffold/ruby_overlay/main.rb", __dir__)
eval(File.read(overlay_path).sub('require_relative "boot"', ""), TOPLEVEL_BINDING, overlay_path)

module Main
  class << self
    attr_accessor :descriptor
  end

  def self.dispatch_core(_env, _stdin)
    descriptor
  end
end

module Tep
  def self.str_hash
    {}
  end
end

native_path = File.expand_path("../runtime/spinel/scaffold/main.rb", __dir__)
native_source = File.read(native_path).split("    out_cookies = controller.cookies.pending\n", 2).last.split("    # Session persistence:", 2).first
eval("module NativeCookieHarness\n  def self.emit(controller, res, request_obj)\n    out_cookies = controller.cookies.pending\n" + native_source + "  end\nend", TOPLEVEL_BINDING, native_path)

class CookieOptionsMergeTest < Minitest::Test
  Controller = Struct.new(:cookies)
  Request = Struct.new(:ssl?)
  Response = Struct.new(:cookies) do
    def set_cookie(name, value, options)
      cookies[name] = [value, options]
    end
  end

  def setup
    @jar = ActionController::CookieJar.new
  end

  def serialized(headers = {})
    secure, same_site, httponly, expires, options = {}, {}, {}, {}, {}
    @jar.pending.each do |name, _value|
      secure[name] = @jar.flag_secure?(name)
      same_site[name] = @jar.flag_samesite(name)
      httponly[name] = @jar.flag_httponly?(name)
      expires[name] = @jar.flag_expires(name)
      options[name] = @jar.options_for(name)
    end
    Main.descriptor = [200, "body", "text/html; charset=utf-8", nil, @jar.pending, {}, secure, same_site, httponly, expires, options]
    _, rack_headers, = Main.run_rack(headers)
    output = StringIO.new
    Main.run(headers, StringIO.new, output)
    native = Response.new({})
    NativeCookieHarness.emit(Controller.new(@jar), native, Request.new(headers["HTTPS"] == "on"))
    [rack_headers.fetch("set-cookie"), output.string, native.cookies]
  end

  def test_explicit_signed_expiry_and_path_override_the_permanent_default
    expires = Time.now.utc + 300
    @jar.signed.permanent[:google_login_state] = {value: "synthetic-nonce", expires: expires, path: "/session", httponly: true, same_site: :lax, secure: true}
    expected = expires.strftime("%a, %d %b %Y %H:%M:%S GMT")
    rack, cgi, native = serialized
    [rack.first, cgi].each do |header|
      assert_includes header, "Path=/session"
      assert_includes header, "Expires=#{expected}"
      assert_includes header, "HttpOnly"
      assert_includes header, "SameSite=Lax"
      assert_includes header, "Secure"
    end
    assert_equal "/session", native["google_login_state"][1]["Path"]
    assert_equal expected, native["google_login_state"][1]["Expires"]
    assert_equal "synthetic-nonce", @jar.signed[:google_login_state]
  end

  def test_deletion_keeps_the_path_and_immediately_expires_every_serializer
    @jar.signed.permanent[:google_login_state] = {value: "synthetic-nonce", path: "/session", httponly: true, secure: true}
    @jar.delete(:google_login_state)
    rack, cgi, native = serialized
    [rack.first, cgi].each do |header|
      assert_includes header, "google_login_state=;"
      assert_includes header, "Path=/session"
      assert_includes header, "Max-Age=0"
      assert_includes header, "Expires=Thu, 01 Jan 1970 00:00:00 GMT"
    end
    assert_equal "0", native["google_login_state"][1]["Max-Age"]
    assert_equal "Thu, 01 Jan 1970 00:00:00 GMT", native["google_login_state"][1]["Expires"]
    assert_nil @jar.signed[:google_login_state]
  end

  def test_bare_permanent_writes_keep_the_upstream_expiry_in_every_serializer
    @jar.permanent[:last_room] = 7
    expected = @jar.flag_expires(:last_room)
    rack, cgi, native = serialized
    assert_includes rack.first, "Expires=#{expected}"
    assert_includes cgi, "Expires=#{expected}"
    assert_equal expected, native["last_room"][1]["Expires"]
    assert_equal (Time.now.utc.year + 20).to_s, expected.split(" ")[3]
  end

  def test_plain_permanent_options_keep_value_and_upstream_none_requires_secure_guard
    @jar.permanent[:selection] = {value: "synthetic", httponly: false, same_site: :none, path: "/selection"}
    rack, cgi, native = serialized
    assert_equal "synthetic", @jar[:selection]
    [rack.first, cgi].each do |header|
      assert_includes header, "selection=synthetic;"
      assert_includes header, "SameSite=None"
      assert_includes header, "Secure"
      refute_includes header, "HttpOnly"
    end
    assert native["selection"][1].key?("Secure")
    refute native["selection"][1].key?("HttpOnly")
  end

  def test_https_keeps_the_upstream_secure_guard
    @jar.signed[:session_token] = {value: "synthetic", httponly: true, same_site: :lax, secure: false}
    rack, cgi, native = serialized("HTTPS" => "on")
    assert_includes rack.first, "Secure"
    assert_includes cgi, "Secure"
    assert native["session_token"][1].key?("Secure")
  end

  def test_cgi_prebuilt_cookie_lines_still_use_the_header_injection_guard
    output = StringIO.new
    CgiIo.write_response(output, 200, "body", cookie_headers: ["ok=value; Path=/", "bad=value\r\nX-Injection: yes"])
    assert_includes output.string, "Set-Cookie: ok=value; Path=/"
    refute_includes output.string, "bad=value"
    refute_includes output.string, "X-Injection"
  end
end
