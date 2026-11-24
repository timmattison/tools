class Widget:
    def render(self):
        return "widget"


class TestWidget:
    def test_render(self):
        assert Widget().render() == "widget"
