defmodule Register do
  def helper(values) do
    Enum.sum(values)
  end

  def describe(name) do
    "the register #{name}"
  end
end
