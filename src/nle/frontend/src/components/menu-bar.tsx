import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type { LoadedRecording } from "../bindings";

interface MenuBarProps {
  onRecordingLoaded: (result: LoadedRecording) => void;
  onError: (message: string) => void;
}

export function MenuBar({ onRecordingLoaded, onError }: MenuBarProps) {
  async function handleOpen() {
    try {
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
        onRecordingLoaded(result);
      }
    } catch (err: unknown) {
      const message = err instanceof Error ? err.message : String(err);
      onError(`Failed to open recording: ${message}`);
    }
  }

  return (
    <div className="menu-bar">
      <button onClick={() => void handleOpen()}>Open Recording</button>
    </div>
  );
}
