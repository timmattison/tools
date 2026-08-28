export function double(x) {
  return x * 2;
}

it.each([
  [1, 2],
  [2, 4],
])("doubles %i", (input, expected) => {
  expect(double(input)).toBe(expected);
});
