import { useState } from "react";
import { MenuBar } from "./components/menu-bar";
import { PreviewPane } from "./components/preview-pane";
import { Timeline } from "./components/timeline";
import { PropertiesPanel } from "./components/properties-panel";
import type { LoadedRecording } from "./bindings";

export function App() {
  const [loaded, setLoaded] = useState<LoadedRecording | null>(null);
  const [error, setError] = useState<string | null>(null);

  function handleRecordingLoaded(result: LoadedRecording) {
    setLoaded(result);
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
          <button onClick={() => { setError(null); }} className="error-dismiss">
            Dismiss
          </button>
        </div>
      )}
      <PreviewPane recording={loaded?.recording ?? null} />
      <Timeline recording={loaded?.recording ?? null} />
      <PropertiesPanel metadata={loaded?.metadata ?? null} />
    </div>
  );
}
