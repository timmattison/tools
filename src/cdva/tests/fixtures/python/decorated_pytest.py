import pytest


def double(x):
    return x * 2


@pytest.mark.parametrize("value,expected", [(1, 2), (2, 4)])
def check_double(value, expected):
    assert double(value) == expected
