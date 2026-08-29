export function add(a, b) {
  return a + b;
}

describe("add", () => {
  it("sums its arguments", () => {
    expect(add(1, 2)).toBe(3);
  });
});
