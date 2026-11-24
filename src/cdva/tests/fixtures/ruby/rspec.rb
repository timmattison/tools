# Calculator adds two numbers.
class Calculator
  def add(a, b)
    a + b
  end
end

RSpec.describe Calculator do
  it "adds two numbers" do
    expect(Calculator.new.add(1, 2)).to eq(3)
  end
end
