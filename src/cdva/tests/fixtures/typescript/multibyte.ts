export function greet(): string {
  return "こんにちは";
}

describe("挨拶", (): void => {
  it("は日本語である", (): void => {
    expect(greet()).toBe("こんにちは");
  });
});
