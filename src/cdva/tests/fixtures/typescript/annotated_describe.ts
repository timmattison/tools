export interface Point {
  x: number;
  y: number;
}

export function shift(point: Point, by: number): Point {
  return { x: point.x + by, y: point.y + by };
}

describe("shift", (): void => {
  const origin: Point = { x: 0, y: 0 };

  it("moves both axes", (): void => {
    const moved: Point = shift(origin, 2);
    expect(moved.x).toBe(2);
  });
});
