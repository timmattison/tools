import unittest


def parse(text):
    return text.strip()


@unittest.skip("not ready")
def test_parse():
    assert parse(" x ") == "x"
