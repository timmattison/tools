export function trim(value: string): string {
  return value.trim();
}

it.only("trims both ends", (): void => {
  expect(trim("  x  ")).toBe("x");
});
