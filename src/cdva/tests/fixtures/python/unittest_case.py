import unittest


class Ledger:
    def total(self):
        return 0


class LedgerCase(unittest.TestCase):
    def test_total(self):
        self.assertEqual(Ledger().total(), 0)
