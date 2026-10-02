require "json"

class BufferedLog
  def initialize
    @pending, @visible = +"", +""
  end
  def write(value) = @pending << value
  def flush
    @visible << @pending
    @pending.clear
  end
  def string = @visible
end

source = File.read(File.expand_path("../runtime/spinel/scaffold/main.rb", __dir__))
methods = source.split("  def self.dispatch(req, res)\n", 2).last.split("  def self.dispatch_request(req, res)\n", 2).first
eval("module Main\n  def self.dispatch(req, res)\n" + methods + "end", TOPLEVEL_BINDING)

module Sock
  def self.sphttp_filesize(_path) = 4096
end
Req = Struct.new(:verb, :path)
Res = Struct.new(:status, :body, :file_path, :upgrading_ws)
module Main
  def self.dispatch_request(req, res)
    case req.path
    when "/explode" then raise "failure"
    when "/assets/app.js" then res.file_path = "static/assets/app.js"
    when "/cable" then res.upgrading_ws = true
    when "/missing" then res.status = 404
    end
  end
end

ENV["RH_REQUEST_METRICS"] = "1"
output, previous = BufferedLog.new, $stdout
begin
  $stdout = output
  ["/join/private-code?email=secret", "/session/transfers/private-token", "/rooms/42/private-bot/messages", "/assets/app.js", "/cable", "/missing", "/explode"].each do |path|
    Main.dispatch(Req.new("GET", path), Res.new(200, "hé", "", false))
  rescue RuntimeError
  end
ensure
  $stdout = previous
end
events = output.string.lines.map { |line| JSON.parse(line).fetch("rh_request") }
raise "missing requests" unless events.length == 7
raise "leaked credential" if output.string.match?(/private-|secret|email=/)
raise "wrong status" unless events.last["status"] == 500 && events[-2]["status"] == 404 && events[-3]["status"] == 101
raise "wrong byte count" unless events[0]["bytes"] == 3 && events[3]["bytes"] == 4096
raise "wrong route" unless events[2]["path"] == "/rooms/:id/:redacted/messages"
raise "invalid duration" unless events.all? { |event| event["duration_ms"] >= 0 }
puts "Request metrics: 7 requests, status/bytes and credential redaction verified"
