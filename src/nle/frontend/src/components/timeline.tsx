import type { Recording } from "../bindings";

interface TimelineProps {
  recording: Recording | null;
}

export function Timeline({ recording }: TimelineProps) {
  if (!recording) {
    return (
      <div className="timeline">
        <div className="placeholder">No recording loaded</div>
      </div>
    );
  }

  const minutes = Math.floor(recording.duration / 60);
  const seconds = (recording.duration % 60).toFixed(1);

  return (
    <div className="timeline">
      <h3>Timeline</h3>
      <div className="placeholder">
        Timeline editor — coming in #150
        <br />
        {recording.events.length} events | {minutes}m {seconds}s
      </div>
    </div>
  );
}
