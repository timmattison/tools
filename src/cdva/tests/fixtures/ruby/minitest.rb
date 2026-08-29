require "minitest/autorun"

# Register records amounts.
class Register
  def total
    0
  end
end

class RegisterCheck < Minitest::Test
  def test_total_starts_at_zero
    assert_equal 0, Register.new.total
  end
end
