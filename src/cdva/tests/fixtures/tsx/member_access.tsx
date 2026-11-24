export function Chart({ canvas, points }: { canvas: HTMLCanvasElement; points: number[] }) {
  const context = canvas.getContext("2d") as CanvasRenderingContext2D;
  context.beginPath();
  context.fillRect(
    0,
    0,
    points.length,
    points.length,
  );
  context.stroke();
  return <canvas className="chart" data-points={points.length} />;
}

export function firstLabel(labels: Iterable<string>) {
  const it = labels[Symbol.iterator]();
  return it.next().value;
}

export function Labels({ labels }: { labels: string[] }) {
  const test = new RegExp("^[a-z]+$");
  const kept = labels.filter((label) => test.test(label));
  return (
    <ul>
      {kept.map((label) => (
        <li key={label}>{label}</li>
      ))}
    </ul>
  );
}

describe.skip("Chart", (): void => {
  test("renders the chart", (): void => {
    expect(Chart).toBeDefined();
  });
});
