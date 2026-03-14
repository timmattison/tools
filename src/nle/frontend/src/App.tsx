import { useState } from "react";
import { MenuBar } from "./components/menu-bar";
import { PreviewPane } from "./components/preview-pane";
import { Timeline } from "./components/timeline";
import { PropertiesPanel } from "./components/properties-panel";
import type { Recording, RecordingMetadata } from "./bindings";

export function App() {
  const [recording, setRecording] = useState<Recording | null>(null);
  const [metadata, setMetadata] = useState<RecordingMetadata | null>(null);
  const [error, setError] = useState<string | null>(null);

  function handleRecordingLoaded(rec: Recording, meta: RecordingMetadata) {
    setRecording(rec);
    setMetadata(meta);
    setError(null);
  }

  function handleError(message: string) {
    setError(message);
  }

  return (
    <div className="app-layout">
      <MenuBar onRecordingLoaded={handleRecordingLoaded} onError={handleError} />
      {error && (
        <div className="error-banner" role="alert">
          {error}
          <button onClick={() => setError(null)} className="error-dismiss">
            Dismiss
          </button>
        </div>
      )}
      <PreviewPane recording={recording} />
      <Timeline recording={recording} />
      <PropertiesPanel metadata={metadata} />
    </div>
  );
}
