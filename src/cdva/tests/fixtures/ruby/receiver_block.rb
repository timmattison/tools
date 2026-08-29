# Each block below belongs to a method of the ledger, and a receiver of its
# own stands in front of the call, so no row of the class is test code.
class Ledger
  def report(name)
    logger.context(name) do |scope|
      scope.write("totals")
      scope.flush
    end
  end

  def publish(name)
    report.feature(name) do |page|
      page.render
    end
  end
end

RSpec.describe Ledger do
  it "reports totals" do
    expect(Ledger.new.report("totals")).to eq(nil)
  end
end
