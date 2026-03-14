import { useState } from "react";
import { MenuBar } from "./components/menu-bar";
import { PreviewPane } from "./components/preview-pane";
import { Timeline } from "./components/timeline";
import { PropertiesPanel } from "./components/properties-panel";
import type { Recording, RecordingMetadata } from "./bindings";

export function App() {
  const [recording, setRecording] = useState<Recording | null>(null);
  const [metadata, setMetadata] = useState<RecordingMetadata | null>(null);

  function handleRecordingLoaded(rec: Recording, meta: RecordingMetadata) {
    setRecording(rec);
    setMetadata(meta);
  }

  return (
    <div className="app-layout">
      <MenuBar onRecordingLoaded={handleRecordingLoaded} />
      <PreviewPane recording={recording} />
      <Timeline recording={recording} />
      <PropertiesPanel metadata={metadata} />
    </div>
  );
}
