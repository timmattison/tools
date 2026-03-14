import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type { LoadedRecording, Recording, RecordingMetadata } from "../bindings";

interface MenuBarProps {
  onRecordingLoaded: (recording: Recording, metadata: RecordingMetadata) => void;
}

export function MenuBar({ onRecordingLoaded }: MenuBarProps) {
  async function handleOpen() {
    const selected = await open({
      multiple: false,
      filters: [
        { name: "Recordings", extensions: ["json", "gz"] },
        { name: "All Files", extensions: ["*"] },
      ],
    });

    if (selected) {
      const result = await invoke<LoadedRecording>("load_recording", {
        path: selected,
      });
      onRecordingLoaded(result.recording, result.metadata);
    }
  }

  return (
    <div className="menu-bar">
      <button onClick={handleOpen}>Open Recording</button>
    </div>
  );
}
