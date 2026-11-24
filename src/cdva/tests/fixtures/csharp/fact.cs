using Xunit;

namespace Example.Calc
{
    public class Calculator
    {
        public int Add(int a, int b)
        {
            return a + b;
        }
    }

    public class AdditionFacts
    {
        [Fact]
        public void AddsTwoNumbers()
        {
            Assert.Equal(3, new Calculator().Add(1, 2));
        }
    }
}
