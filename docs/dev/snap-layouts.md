# Snap Layouts on the own maximize button

How the Windows 11 Snap Layouts flyout appears on Lethora's own maximize
button. Status: built 2026-10-16 (title bar part T2), Windows only. Code:
`app/src-tauri/src/snap.rs`, `snapLayouts` in `app/src/chrome.ts`.

## 1. The problem

Windows 11 shows the Snap Layouts flyout while the mouse rests on a window's
maximize button. It learns where that button is from one message: the
window under the mouse answers `WM_NCHITTEST` with `HTMAXBUTTON`.

On Windows and Linux the window is frameless (`decorations: false`), and the
minimize / maximize / close buttons are web content (`WindowButtons` in
`chrome.ts`). WebView2 runs in its own child window that covers the whole
client area and takes all mouse input, so the hit test never reaches the
Tauri window. No HTML, CSS or Tauri API can produce `HTMAXBUTTON`, so the
web button alone never gets the flyout. Hover styles still work.

## 2. The mechanism

A small transparent native child window (the *overlay*) lies exactly over
the web maximize button:

```
Tauri window (top level, frameless)
├── WebView2 container (fills the client area): the title-bar row and buttons
└── overlay (HWND_TOP, over the web maximize button): answers HTMAXBUTTON
```

- **Hit test:** the overlay answers `WM_NCHITTEST` with `HTMAXBUTTON`, so
  Windows treats it as the maximize button and shows the flyout after its
  usual delay.
- **Invisible:** it draws nothing (null brush), so the web button shows
  through. It must not be `WS_EX_LAYERED` or `WS_EX_TRANSPARENT`, because
  both make the hit test pass through it. `WS_CLIPSIBLINGS` and `HWND_TOP`
  keep it above the WebView2 container.
- **Mouse to the web view:** the overlay now gets the mouse there instead of
  the web button, so it forwards what happens as the `snap-maximize` event:

  | Message | Event | Web view |
  | --- | --- | --- |
  | `WM_NCMOUSEMOVE` (first) | `hover` | adds class `hover` |
  | `WM_NCLBUTTONDOWN`, `WM_NCLBUTTONDBLCLK` | `press` | adds class `pressed` |
  | `WM_NCLBUTTONUP` after a press | `click` | removes `pressed`, clicks the button |
  | `WM_NCMOUSELEAVE` | `leave` | removes `hover` and `pressed` |

  `TrackMouseEvent(TME_LEAVE | TME_NONCLIENT)` is re-armed on every move to
  get the leave message. Non-client double clicks arrive whatever the class
  style, so they count as a press. Button presses never reach
  `DefWindowProcW`, which would start its own caption-button tracking.
- **Same look:** `.hover` and `.pressed` share the `:hover` and `:active`
  rules of `.bar-window button` in `style.css`.
- **Same action:** the click goes through the web button's own handler
  (`toggleMaximize` and its error handling), so maximize and restore work as
  before, as does the icon switch on resize.

## 3. Keeping the overlay on the button

The web view reports where the button is. `snapLayouts` sends the button's
`getBoundingClientRect()` times `devicePixelRatio`, which includes both
monitor DPI and WebView2 zoom, to the `snap_rect(x, y, width, height)`
command. Each edge is rounded on its own so the rectangle stays
pixel-exact. The coordinates are physical pixels from the web view's top
left, which is the window's client area.

It reports at start, on `resize` (window size, maximize/restore), from a
`ResizeObserver` on the button, and when `devicePixelRatio` changes (a
`matchMedia("(resolution: …dppx)")` listener, set up again for each new
ratio). Unchanged rectangles are not sent again. An empty rectangle hides
the overlay.

`snap_rect` is a sync command, so Tauri runs it on the main thread, in
order. The overlay is created there on the first report, so the main event
loop pumps its messages. Its state (window handle, hover, pressed) lives in
main-thread `thread_local`s. Nothing subclasses the Tauri window.

## 4. Platforms

- **Windows 11:** flyout, hover/pressed look and click, at any scaling and
  zoom, also while maximized.
- **Windows 10:** same code; the hover and click work, and Windows shows no
  flyout.
- **Linux:** no overlay; the module and command are `cfg(windows)` and the
  web side runs only when the user agent says Windows.
- **macOS:** native title bar and buttons, unchanged.
- **Browser dev mock:** `snap_rect` is a no-op in `devmock.ts`.

The only dependency is the `windows` crate 0.62, as a Windows-only
dependency. Tauri, tao and wry already build it with every feature used
here, so nothing new gets compiled.

## 5. Known limitations

- The maximize button shows no tooltip on Windows, because the overlay takes
  the mouse.
- During a live resize the overlay follows one report behind. That only
  matters for hover, and the next report fixes it.
- Anything the web view draws over the maximize button (a full-window
  backdrop) still has the overlay on top in that spot.

## 6. Options not taken

- **WebView2 non-client regions** (`IsNonClientRegionSupportEnabled`, CSS
  `app-region`): wry 0.57 already turns it on, but with windowed hosting it
  covers only `app-region: drag`. The caption-button region kinds
  (minimize / maximize / close) need WebView2 visual (composition) hosting,
  which wry does not use.
- **tauri-plugin-decorum:** unmaintained since 2024-09, and it fakes Win+Z
  with simulated key presses. Rejected 2026-10-16.
- **tauri-plugin-frame** (1.1.8): uses the same overlay idea, but it injects
  its own buttons and script and computes the overlay's position from fixed
  button sizes instead of measuring the DOM. That would clash with
  `WindowButtons` and with zoom.
- **Subclassing the Tauri window's `WM_NCHITTEST`:** never sees the mouse,
  because the WebView2 child answers first.

References: [Microsoft: snap layouts for custom title
bars](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/ui/apply-snap-layout-menu),
[Snap Layouts in a frameless Tauri
app](https://dev.to/zbrooklyn/windows-11-snap-layouts-in-a-frameless-tauri-app-the-part-every-guide-gets-wrong-2fjh),
[Zbrooklyn/tauri-snap-layouts](https://github.com/Zbrooklyn/tauri-snap-layouts).
