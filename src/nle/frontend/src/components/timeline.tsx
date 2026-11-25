import type { Recording } from "../bindings";
import { formatDuration } from "../format-duration";

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

  return (
    <div className="timeline">
      <h3>Timeline</h3>
      <div className="placeholder">
        Timeline editor — coming in #150
        <br />
        {recording.events.length} events | {formatDuration(recording.duration)}
      </div>
    </div>
  );
}
