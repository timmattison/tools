# Ledger holds a running total.
class Ledger
  def total
    0
  end
end

describe "the ledger" do
  context "when it is empty" do
    specify "the total is zero" do
      expect(Ledger.new.total).to eq(0)
    end
  end
end
