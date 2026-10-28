// The splash window's page (splash.html): shows the progress main.ts reports
// (startup.ts) and closes the window when the app is ready.

import "./style.css";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { SPLASH_CLOSE, SPLASH_PROGRESS, type SplashProgress } from "./startup.ts";

const root = document.getElementById("splash")!;
const bar = document.getElementById("splash-bar")!;
const status = document.getElementById("splash-status")!;

getVersion().then((version) => (document.getElementById("splash-version")!.textContent = `Version ${version}`), () => undefined);

void listen<SplashProgress>(SPLASH_PROGRESS, (e) => {
  const percent = Math.max(0, Math.min(100, e.payload.percent));
  bar.style.width = `${percent}%`;
  root.setAttribute("aria-valuenow", String(Math.round(percent)));
  if (e.payload.message !== undefined) status.textContent = e.payload.message;
});
void listen(SPLASH_CLOSE, () => void getCurrentWindow().close());
