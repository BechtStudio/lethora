// Screenshots for the user manual: builds the browser preview (mock backend),
// serves it with `vite preview`, drives headless Edge over the Chrome DevTools
// Protocol and writes docs/images/<name>.png for each shot in shots.mjs.
//
//   node scripts/screenshots.mjs [--only name,...] [--no-build]
//
// --no-build reuses the last build (in the system temp folder). Edge is
// LETHORA_EDGE or the default install path. Node 24, no dependencies.

import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { shots } from "./shots.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const APP = path.join(ROOT, "app");
const VITE = path.join(APP, "node_modules", "vite", "bin", "vite.js");
const DIST = path.join(os.tmpdir(), "lethora-screenshots-ui");
const OUT = path.join(ROOT, "docs", "images");
const EDGE = process.env.LETHORA_EDGE ?? "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const ACCENT = "#d9480f";

const args = process.argv.slice(2);
const only = args.includes("--only") ? args[args.indexOf("--only") + 1].split(",") : null;
const build = !args.includes("--no-build");

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
/** Vite and Edge, ended at exit (also on Ctrl+C). */
const children = [];
process.on("SIGINT", () => {
  for (const c of children) kill(c);
  process.exit(130);
});

function freePort() {
  return new Promise((resolve, reject) => {
    const s = net.createServer().once("error", reject);
    s.listen(0, "127.0.0.1", () => {
      const { port } = s.address();
      s.close(() => resolve(port));
    });
  });
}

/** Ends a process and its children (Edge and Vite start helpers). */
function kill(child) {
  if (!child || child.exitCode !== null) return;
  if (process.platform === "win32") spawnSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore" });
  else child.kill("SIGKILL");
}

async function waitFor(what, test, ms = 30_000) {
  const end = Date.now() + ms;
  for (;;) {
    try {
      const v = await test();
      if (v) return v;
    } catch {
      // not yet
    }
    if (Date.now() > end) throw new Error(`Timed out waiting for ${what}.`);
    await sleep(100);
  }
}

/** A minimal CDP client over one WebSocket. */
class Cdp {
  constructor(ws) {
    this.ws = ws;
    this.id = 0;
    this.pending = new Map();
    this.listeners = [];
    ws.addEventListener("message", (e) => {
      const m = JSON.parse(e.data);
      if (m.id !== undefined) {
        const p = this.pending.get(m.id);
        this.pending.delete(m.id);
        if (m.error) p?.reject(new Error(`${p.method}: ${m.error.message}`));
        else p?.resolve(m.result);
      } else for (const l of this.listeners) l(m);
    });
  }

  static async connect(url) {
    const ws = new WebSocket(url);
    await new Promise((resolve, reject) => {
      ws.addEventListener("open", resolve, { once: true });
      ws.addEventListener("error", () => reject(new Error(`Cannot connect to ${url}`)), { once: true });
    });
    return new Cdp(ws);
  }

  send(method, params = {}) {
    const id = ++this.id;
    this.ws.send(JSON.stringify({ id, method, params }));
    return new Promise((resolve, reject) => this.pending.set(id, { resolve, reject, method }));
  }

  once(method) {
    return new Promise((resolve) => {
      const l = (m) => {
        if (m.method !== method) return;
        this.listeners.splice(this.listeners.indexOf(l), 1);
        resolve(m.params);
      };
      this.listeners.push(l);
    });
  }

  /** Evaluates `expression` in the page (awaiting promises) and returns its value. */
  async eval(expression) {
    const r = await this.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(`In the page: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}\n  ${expression}`);
    return r.result.value;
  }
}

/** The element's box (CSS px), the first match of `selector` containing `text` if given. */
const boxOf = (cdp, selector, text) =>
  cdp.eval(`(() => {
    const el = [...document.querySelectorAll(${JSON.stringify(selector)})].find((e) => ${JSON.stringify(text ?? "")} === "" || e.textContent.includes(${JSON.stringify(text ?? "")}));
    if (!el) return null;
    el.scrollIntoView({ block: "nearest", inline: "nearest" });
    const r = el.getBoundingClientRect();
    return { x: r.left, y: r.top, width: r.width, height: r.height };
  })()`);

async function mouse(cdp, type, x, y) {
  await cdp.send("Input.dispatchMouseEvent", { type, x, y, button: type === "mouseMoved" ? "none" : "left", clickCount: type === "mouseMoved" ? 0 : 1 });
}

/** Runs one step: a JS string, or { click | hover | wait: selector, text? }, { key, ctrl?, shift?, alt? }, { sleep: ms }. */
async function runStep(cdp, step) {
  if (typeof step === "string") return cdp.eval(`(async () => { ${step} })()`);
  if (step.sleep !== undefined) return sleep(step.sleep);
  if (step.wait) return waitFor(step.wait, () => boxOf(cdp, step.wait, step.text), step.timeout ?? 15_000);
  if (step.key) {
    const init = JSON.stringify({ key: step.key, ctrlKey: !!step.ctrl, shiftKey: !!step.shift, altKey: !!step.alt, bubbles: true, cancelable: true });
    return cdp.eval(`(() => { const t = document.activeElement ?? document.body; t.dispatchEvent(new KeyboardEvent("keydown", ${init})); t.dispatchEvent(new KeyboardEvent("keyup", ${init})); })()`);
  }
  const selector = step.click ?? step.hover;
  if (!selector) throw new Error(`Unknown step: ${JSON.stringify(step)}`);
  const box = await waitFor(selector, () => boxOf(cdp, selector, step.text), 15_000);
  const x = box.x + (step.x ?? box.width / 2);
  const y = box.y + (step.y ?? box.height / 2);
  await mouse(cdp, "mouseMoved", x, y);
  if (step.click) {
    await mouse(cdp, "mousePressed", x, y);
    await mouse(cdp, "mouseReleased", x, y);
  }
}

/** Numbered circles at the callouts' elements (at a corner or edge, + dx/dy), optional outlines. */
async function annotate(cdp, callouts) {
  const spec = JSON.stringify(callouts);
  await cdp.eval(`(() => {
    const layer = document.createElement("div");
    layer.id = "shot-callouts";
    layer.style.cssText = "position:fixed;inset:0;pointer-events:none;z-index:2147483647";
    for (const c of ${spec}) {
      const el = [...document.querySelectorAll(c.selector)].find((e) => !c.text || e.textContent.includes(c.text));
      if (!el) throw new Error("No element for callout " + c.n + ": " + c.selector);
      const r = el.getBoundingClientRect();
      if (c.outline) {
        const o = document.createElement("div");
        o.style.cssText = "position:fixed;border:2px solid ${ACCENT};border-radius:6px;box-shadow:0 0 0 3px rgba(255,255,255,.6)";
        Object.assign(o.style, { left: r.left - 4 + "px", top: r.top - 4 + "px", width: r.width + 4 + "px", height: r.height + 4 + "px" });
        layer.append(o);
      }
      const d = document.createElement("div");
      d.textContent = c.n;
      d.style.cssText = "position:fixed;width:26px;height:26px;margin:-13px 0 0 -13px;border-radius:50%;background:${ACCENT};color:#fff;font:700 14px/26px system-ui,sans-serif;text-align:center;box-shadow:0 0 0 2px #fff,0 1px 4px rgba(0,0,0,.4)";
      const cx = (r.left + r.right) / 2;
      const cy = (r.top + r.bottom) / 2;
      const at = { "top-right": [r.right, r.top], top: [cx, r.top], bottom: [cx, r.bottom], left: [r.left, cy], right: [r.right, cy], center: [cx, cy] }[c.at ?? "top-right"];
      Object.assign(d.style, { left: at[0] + (c.dx ?? 0) + "px", top: at[1] + (c.dy ?? 0) + "px" });
      layer.append(d);
    }
    document.body.append(layer);
  })()`);
}

async function takeShot(cdp, base, shot) {
  const [width, height] = shot.viewport ?? [1400, 850];
  await cdp.send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: shot.scale ?? 1, mobile: false });
  await cdp.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: shot.scheme ?? "light" }] });
  // Fresh preferences before the app reads them (only for this navigation).
  const { identifier } = await cdp.send("Page.addScriptToEvaluateOnNewDocument", {
    source: `try { localStorage.clear(); for (const [k, v] of Object.entries(${JSON.stringify(shot.storage ?? {})})) localStorage.setItem(k, v); } catch {}`,
  });
  const loaded = cdp.once("Page.loadEventFired");
  await cdp.send("Page.navigate", { url: base + (shot.query ?? "") });
  await loaded;
  await cdp.send("Page.removeScriptToEvaluateOnNewDocument", { identifier });
  await waitFor("the editor", () => cdp.eval(`!!document.querySelector(".cm-content") && document.getElementById("project-name").textContent !== ""`));
  for (const step of shot.steps ?? []) await runStep(cdp, step);
  // Let transitions and page images settle.
  await sleep(shot.settle ?? 600);
  if (shot.callouts?.length) await annotate(cdp, shot.callouts);
  const params = { format: "png" };
  if (shot.clip) {
    const box = await boxOf(cdp, shot.clip.selector ?? shot.clip);
    if (!box) throw new Error(`No element to clip to: ${shot.clip.selector ?? shot.clip}`);
    const pad = shot.clip.padding ?? 0;
    const x = Math.max(0, box.x - pad);
    const y = Math.max(0, box.y - pad);
    params.clip = { x, y, width: Math.min(width - x, box.width + 2 * pad), height: Math.min(height - y, box.height + 2 * pad), scale: 1 };
  }
  const { data } = await cdp.send("Page.captureScreenshot", params);
  const file = path.join(OUT, `${shot.name}.png`);
  fs.writeFileSync(file, Buffer.from(data, "base64"));
  console.log(`${path.relative(ROOT, file)}  ${(fs.statSync(file).size / 1024).toFixed(0)} KB`);
}

async function main() {
  const todo = only ? shots.filter((s) => only.includes(s.name)) : shots;
  if (only && todo.length !== only.length) throw new Error(`Unknown shot in --only: ${only.filter((n) => !shots.some((s) => s.name === n)).join(", ")}`);
  if (build) {
    console.log("Building the browser preview…");
    const r = spawnSync(process.execPath, [VITE, "build", "--mode", "development", "--outDir", DIST, "--emptyOutDir"], {
      cwd: APP,
      env: { ...process.env, NODE_ENV: "development" },
      stdio: ["ignore", "ignore", "inherit"],
    });
    if (r.status !== 0) throw new Error("The build failed.");
  } else if (!fs.existsSync(path.join(DIST, "index.html"))) throw new Error(`No build in ${DIST}; run without --no-build.`);
  fs.mkdirSync(OUT, { recursive: true });

  const profile = fs.mkdtempSync(path.join(os.tmpdir(), "lethora-edge-"));
  let cdp;
  try {
    const port = await freePort();
    const vite = spawn(process.execPath, [VITE, "preview", "--outDir", DIST, "--host", "127.0.0.1", "--port", String(port), "--strictPort"], { cwd: APP, stdio: "ignore" });
    children.push(vite);
    const base = `http://127.0.0.1:${port}/`;
    await waitFor("vite preview", async () => (await fetch(base)).ok);

    const edge = spawn(
      EDGE,
      ["--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "--no-first-run", "--no-default-browser-check", "--disable-extensions", "--hide-scrollbars", "--force-color-profile=srgb", "about:blank"],
      { stdio: "ignore" },
    );
    children.push(edge);
    const devtools = path.join(profile, "DevToolsActivePort");
    const cdpPort = await waitFor("Edge", () => fs.existsSync(devtools) && fs.readFileSync(devtools, "utf8").split("\n")[0]);
    const page = await waitFor("an Edge page", async () => (await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json()).find((t) => t.type === "page"));
    cdp = await Cdp.connect(page.webSocketDebuggerUrl);
    await cdp.send("Page.enable");
    await cdp.send("Runtime.enable");
    for (const shot of todo) await takeShot(cdp, base, shot);
  } finally {
    try {
      await Promise.race([cdp?.send("Browser.close"), sleep(2000)]);
    } catch {
      // killed below
    }
    cdp?.ws.close();
    for (const c of children.reverse()) kill(c);
    await sleep(300);
    fs.rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
  }
}

main().catch((e) => {
  console.error(e.message ?? e);
  process.exitCode = 1;
});
