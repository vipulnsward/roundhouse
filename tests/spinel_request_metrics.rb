require "json"
require "open3"
require "tmpdir"

root = File.expand_path("..", __dir__)
source = File.read("#{root}/runtime/spinel/scaffold/main.rb")
dispatch = source.split("  def self.dispatch(req, res)\n", 2).last.split("  def self.dispatch_request(req, res)\n", 2).first
program = <<~RUBY
  require "json"
  class Request
    def initialize(fail); @fail = fail; end
    def fail?; @fail; end
    def verb; "GET"; end
    def path; @fail ? "/rooms/1/messages/19/edit" : "/up"; end
  end
  class Response
    attr_accessor :status, :body, :file_path, :upgrading_ws
    def initialize
      @status = 200
      @body = "ok"
      @file_path = ""
      @upgrading_ws = false
    end
  end
  module Sock
    def self.sphttp_filesize(path); 0; end
  end
  module Main
    def self.dispatch_request(req, res)
      raise NoMethodError, "private helper" if req.fail?
    end
    def self.dispatch(req, res)
  #{dispatch}
  end
  begin
    Main.dispatch(Request.new(true), Response.new)
  rescue StandardError
    puts "rescued"
  end
  Main.dispatch(Request.new(false), Response.new)
RUBY

Dir.mktmpdir("request-metrics") do |dir|
  script, binary = "#{dir}/probe.rb", "#{dir}/probe"
  File.write(script, program)
  out, err, status = Open3.capture3(ENV.fetch("SPINEL", "spinel"), script, "-o", binary)
  abort "native compile failed: #{out}\n#{err}" unless status.success?
  out, err, status = Open3.capture3({ "RH_REQUEST_METRICS" => "1" }, binary)
  abort "native probe failed: #{out}\n#{err}" unless status.success? && out.include?("rescued")
  events = out.lines.filter_map { |line| JSON.parse(line)["rh_request"] if line.start_with?("{") }
  abort "expected one HTTP 500 and one HTTP 200 event, got #{events.inspect}" unless events.map { |event| event["status"] } == [500, 200]
  abort "request path was not normalized" unless events.first["path"] == "/rooms/:id/messages/:id/edit"
  abort "wrong error response size" unless events.first["bytes"] == "internal server error".bytesize
  abort "wrong successful response size" unless events.last["bytes"] == 2
  puts "native exception telemetry passes"
end
