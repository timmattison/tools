import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type { LoadedRecording } from "../bindings";

interface MenuBarProps {
  onRecordingLoaded: (result: LoadedRecording) => void;
  onError: (message: string) => void;
}

export function MenuBar({ onRecordingLoaded, onError }: MenuBarProps) {
  // True from the click until the dialog and the load end. The disabled
  // button starts no second load that can end last and win.
  const [opening, setOpening] = useState(false);

  async function handleOpen() {
    setOpening(true);
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
    } finally {
      setOpening(false);
    }
  }

  return (
    <div className="menu-bar">
      <button disabled={opening} onClick={() => void handleOpen()}>
        Open Recording
      </button>
    </div>
  );
}
