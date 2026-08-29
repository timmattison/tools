defmodule Greeter do
  def greet do
    "こんにちは"
  end
end

defmodule GreeterChecks do
  use ExUnit.Case

  test "日本語で挨拶する" do
    assert Greeter.greet() == "こんにちは"
  end
end
