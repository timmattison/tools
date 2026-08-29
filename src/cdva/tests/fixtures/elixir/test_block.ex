defmodule Calc do
  def add(a, b) do
    a + b
  end
end

defmodule CalcChecks do
  describe "add/2" do
    test "adds two numbers" do
      assert Calc.add(1, 2) == 3
    end

    property "adds in either order" do
      assert Calc.add(1, 2) == Calc.add(2, 1)
    end
  end
end
