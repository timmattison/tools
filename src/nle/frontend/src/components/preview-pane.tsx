import type { Recording } from "../bindings";

interface PreviewPaneProps {
  recording: Recording | null;
}

export function PreviewPane({ recording }: PreviewPaneProps) {
  if (!recording) {
    return (
      <div className="preview-pane">
        <div className="placeholder">
          Open a recording to preview it here
        </div>
      </div>
    );
  }

  return (
    <div className="preview-pane">
      <div className="placeholder">
        Terminal preview — coming in #151
        <br />
        {recording.width}x{recording.height}
      </div>
    </div>
  );
}
