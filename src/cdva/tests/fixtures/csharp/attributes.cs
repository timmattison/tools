using NUnit.Framework;

namespace Example.Ledger
{
    public class Ledger
    {
        public int Total { get; set; }
    }

    [TestFixture]
    public class LedgerFixture
    {
        private readonly Ledger ledger = new Ledger();

        [SetUp]
        public void Reset()
        {
            ledger.Total = 0;
        }

        [Test]
        public void StartsAtZero()
        {
            Assert.AreEqual(0, ledger.Total);
        }
    }
}
