# Greeter greets in Japanese.
class Greeter
  def greet
    "こんにちは"
  end
end

RSpec.describe Greeter do
  it "日本語で挨拶する" do
    expect(Greeter.new.greet).to eq("こんにちは")
  end
end
