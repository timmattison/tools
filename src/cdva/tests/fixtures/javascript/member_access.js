export function draw(canvas) {
  const context = canvas.getContext("2d");
  context.beginPath();
  context.fillRect(
    0,
    0,
    canvas.width,
    canvas.height,
  );
  context.stroke();
}

export function first(iterable) {
  const it = iterable[Symbol.iterator]();
  return it.next().value;
}

export function lowerCaseOnly(lines) {
  const test = new RegExp("^[a-z]+$");
  return lines.filter((line) => test.test(line));
}

describe.skip("draw", () => {
  test("takes a canvas", () => {
    expect(draw).toBeDefined();
  });
});
