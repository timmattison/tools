defmodule Ledger do
  def total(entries) do
    Enum.sum(entries)
  end

  def record(entries, amount) do
    [amount | entries]
  end
end

defmodule LedgerChecks do
  use ExUnit.Case, async: true

  test "totals the entries it holds" do
    assert Ledger.total([1, 2]) == 3
  end
end
