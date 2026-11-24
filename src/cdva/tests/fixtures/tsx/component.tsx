export function Badge({ label }: { label: string }) {
  return <span className="badge">{label}</span>;
}

describe("Badge", (): void => {
  it("renders the label it was given", (): void => {
    expect(Badge({ label: "ok" })).toBeTruthy();
  });
});
