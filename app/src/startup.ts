// Startup handover between the two windows: the main window starts hidden and
// reports progress to the splash window (splash.ts), then shows itself and
// closes the splash. Without Tauri (browser preview) there is no splash.

import { emitTo } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

export const SPLASH_PROGRESS = "splash-progress";
export const SPLASH_CLOSE = "splash-close";

export interface SplashProgress {
  /** 0–100. */
  percent: number;
  message?: string;
}

/** The splash stays at least this long (ms since the app's script started), so a fast start doesn't flash it. */
const MIN_SPLASH_MS = 1500;

const started = performance.now();
const hasTauri = "__TAURI_INTERNALS__" in window;

/** Shows `percent` and what is happening on the splash (best effort: it is only decoration). */
export function startupProgress(percent: number, message?: string): void {
  if (!hasTauri) return;
  const payload: SplashProgress = { percent, message };
  emitTo("splash", SPLASH_PROGRESS, payload).catch(() => undefined);
}

/** The app is ready: shows the main window and closes the splash. */
export async function startupDone(): Promise<void> {
  if (!hasTauri) return;
  startupProgress(100, "Ready");
  await new Promise((r) => setTimeout(r, Math.max(0, MIN_SPLASH_MS - (performance.now() - started))));
  const win = getCurrentWindow();
  await win.show();
  await win.setFocus();
  await emitTo("splash", SPLASH_CLOSE).catch(() => undefined);
}
