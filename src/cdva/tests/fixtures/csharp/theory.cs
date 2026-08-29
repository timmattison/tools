using Xunit;

namespace Example.Calc
{
    public class Doubling
    {
        public int Doubled(int value)
        {
            return value * 2;
        }

        [Theory]
        [InlineData(1)]
        [InlineData(2)]
        public void DoublesEveryInput(int value)
        {
            Assert.True(Doubled(value) > value);
        }
    }
}
