require_relative "../runtime/ruby/action_controller/message_verifier"
require_relative "../runtime/ruby/active_support_ext"

raise "equal values rejected" unless ActiveSupport::SecurityUtils.secure_compare("nonce", "nonce")
raise "mismatch accepted" if ActiveSupport::SecurityUtils.secure_compare("nonce", "noncf")
raise "length mismatch accepted" if ActiveSupport::SecurityUtils.secure_compare("nonce", "nonce-long")
raise "binary mismatch accepted" if ActiveSupport::SecurityUtils.secure_compare("a\0b", "a\0c")
raise "empty values rejected" unless ActiveSupport::SecurityUtils.secure_compare("", "")
puts "SecurityUtils delegates byte comparisons to the existing verifier"
