export function Greeting() {
  return <p lang="ja">こんにちは</p>;
}

describe("Greeting", (): void => {
  it("は日本語で挨拶する", (): void => {
    expect(Greeting()).toBeTruthy();
  });
});
