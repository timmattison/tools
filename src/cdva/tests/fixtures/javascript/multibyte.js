export function greet() {
  return "こんにちは";
}

describe("挨拶", () => {
  it("は日本語である", () => {
    expect(greet()).toBe("こんにちは");
  });
});
