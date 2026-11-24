export function draw(canvas: HTMLCanvasElement): void {
  const context = canvas.getContext("2d") as CanvasRenderingContext2D;
  context.beginPath();
  context.fillRect(
    0,
    0,
    canvas.width,
    canvas.height,
  );
  context.stroke();
}

export function first<T>(iterable: Iterable<T>): T | undefined {
  const it: Iterator<T> = iterable[Symbol.iterator]();
  return it.next().value;
}

export function lowerCaseOnly(lines: string[]): string[] {
  const test: RegExp = new RegExp("^[a-z]+$");
  return lines.filter((line: string): boolean => test.test(line));
}

describe.skip("draw", (): void => {
  test("takes a canvas", (): void => {
    expect(draw).toBeDefined();
  });
});
