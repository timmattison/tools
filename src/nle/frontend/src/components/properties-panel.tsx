import type { RecordingMetadata } from "../bindings";

interface PropertiesPanelProps {
  metadata: RecordingMetadata | null;
}

export function PropertiesPanel({ metadata }: PropertiesPanelProps) {
  if (!metadata) {
    return (
      <div className="properties-panel">
        <h3>Properties</h3>
        <div className="placeholder">No recording loaded</div>
      </div>
    );
  }

  const date = new Date(metadata.timestamp * 1000);

  return (
    <div className="properties-panel">
      <h3>Properties</h3>
      <div className="metadata-item">
        <span className="metadata-label">Title</span>
        <span className="metadata-value">{metadata.title || "(untitled)"}</span>
      </div>
      <div className="metadata-item">
        <span className="metadata-label">Command</span>
        <span className="metadata-value">{metadata.command}</span>
      </div>
      <div className="metadata-item">
        <span className="metadata-label">Duration</span>
        <span className="metadata-value">{metadata.duration.toFixed(1)}s</span>
      </div>
      <div className="metadata-item">
        <span className="metadata-label">Events</span>
        <span className="metadata-value">{metadata.event_count}</span>
      </div>
      <div className="metadata-item">
        <span className="metadata-label">Size</span>
        <span className="metadata-value">
          {metadata.width}x{metadata.height}
        </span>
      </div>
      <div className="metadata-item">
        <span className="metadata-label">Recorded</span>
        <span className="metadata-value">{date.toLocaleString()}</span>
      </div>
      <div className="metadata-item">
        <span className="metadata-label">File</span>
        <span className="metadata-value" title={metadata.file_path}>
          {metadata.file_path.split(/[/\\]/).pop()}
        </span>
      </div>
    </div>
  );
}
