using Xunit;

namespace Example.Greet
{
    public class Greeter
    {
        public string Greet()
        {
            return "こんにちは";
        }
    }

    public class GreetingFacts
    {
        [Fact]
        public void GreetsInJapanese()
        {
            // 挨拶を確かめる
            Assert.Equal("こんにちは", new Greeter().Greet());
        }
    }
}
