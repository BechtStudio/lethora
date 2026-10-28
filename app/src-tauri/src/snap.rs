//! Windows 11 Snap Layouts on the window's own maximize button.
//!
//! Windows shows the Snap Layouts flyout while the mouse rests on a window's
//! maximize button, which it learns from `WM_NCHITTEST` answering
//! `HTMAXBUTTON`. The window is frameless and its buttons are web content,
//! and WebView2's child window takes all mouse input, so a transparent child
//! window lies over the web maximize button and answers the hit test. It
//! tells the web view what the mouse does there (`snap-maximize` events:
//! `hover`, `leave`, `press`, `click`), which shows it on the button and
//! clicks it; the web view reports where the button is ([`snap_rect`]).
//!
//! The overlay lives on the main thread (where [`snap_rect`] runs), whose
//! event loop pumps its messages.

use std::cell::{Cell, OnceCell};

use tauri::{Emitter, WebviewWindow};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetStockObject, HBRUSH, NULL_BRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, HTMAXBUTTON, HWND_TOP, RegisterClassW, SW_HIDE,
    SWP_NOACTIVATE, SWP_SHOWWINDOW, SetWindowPos, ShowWindow, WINDOW_EX_STYLE, WM_NCHITTEST,
    WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE,
    WM_NCRBUTTONDOWN, WM_NCRBUTTONUP, WNDCLASSW, WS_CHILD, WS_CLIPSIBLINGS,
};
use windows::core::w;

const EVENT: &str = "snap-maximize";

thread_local! {
    /// The overlay, once created (main thread).
    static OVERLAY: Cell<Option<HWND>> = const { Cell::new(None) };
    /// The window the overlay's events go to.
    static WINDOW: OnceCell<WebviewWindow> = const { OnceCell::new() };
    static HOVERING: Cell<bool> = const { Cell::new(false) };
    static PRESSED: Cell<bool> = const { Cell::new(false) };
}

/// Puts the overlay over the maximize button: `x`, `y`, `width` and `height`
/// in physical pixels from the web view's top left (it fills the window's
/// client area); an empty rectangle hides it. Not async, so it runs on the
/// main thread, in order.
#[tauri::command]
pub fn snap_rect(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    window: WebviewWindow,
) -> Result<(), String> {
    if width <= 0 || height <= 0 {
        if let Some(overlay) = OVERLAY.get() {
            // SAFETY: `overlay` is our own window (returns whether it was visible).
            let _ = unsafe { ShowWindow(overlay, SW_HIDE) };
        }
        return Ok(());
    }
    let overlay = match OVERLAY.get() {
        Some(overlay) => overlay,
        None => {
            let overlay = create(&window)?;
            OVERLAY.set(Some(overlay));
            overlay
        }
    };
    // HWND_TOP keeps it above the web view's own child window.
    // SAFETY: `overlay` is our own window.
    unsafe {
        SetWindowPos(
            overlay,
            Some(HWND_TOP),
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
    }
    .map_err(|e| e.to_string())
}

/// Creates the (hidden) overlay as a child of `window`.
fn create(window: &WebviewWindow) -> Result<HWND, String> {
    let parent = window.hwnd().map_err(|e| e.to_string())?;
    WINDOW.with(|w| {
        let _ = w.set(window.clone());
    });
    let class = w!("LethoraSnapOverlay");
    // SAFETY: the class and window names are static strings; `class` outlives
    // the call.
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?.into();
        let class_info = WNDCLASSW {
            lpfnWndProc: Some(overlay_proc),
            hInstance: instance,
            // Draws nothing: the web button shows through.
            hbrBackground: HBRUSH(GetStockObject(NULL_BRUSH).0),
            lpszClassName: class,
            ..Default::default()
        };
        // Fails harmlessly if registered already (a retry after a failed create).
        RegisterClassW(&class_info);
        // Not layered or transparent: either would let the hit test through.
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!(""),
            WS_CHILD | WS_CLIPSIBLINGS,
            0,
            0,
            0,
            0,
            Some(parent),
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|e| e.to_string())
}

/// Sends the mouse's state on the overlay to the web view.
fn emit(state: &str) {
    WINDOW.with(|w| {
        if let Some(window) = w.get() {
            let _ = window.emit(EVENT, state);
        }
    });
}

/// The overlay is the maximize button to Windows; the button presses and
/// clicks go to the web view, never to the default caption-button handling.
unsafe extern "system" fn overlay_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCHITTEST => return LRESULT(HTMAXBUTTON as isize),
        WM_NCMOUSEMOVE => {
            if !HOVERING.replace(true) {
                emit("hover");
            }
            let mut track = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE | TME_NONCLIENT,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            // SAFETY: `track` is a valid TRACKMOUSEEVENT for this call.
            let _ = unsafe { TrackMouseEvent(&mut track) };
        }
        WM_NCMOUSELEAVE => {
            HOVERING.set(false);
            PRESSED.set(false);
            emit("leave");
        }
        // Non-client double clicks arrive whatever the class style: a press.
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK => {
            PRESSED.set(true);
            emit("press");
        }
        WM_NCLBUTTONUP => {
            if PRESSED.replace(false) {
                emit("click");
            }
        }
        WM_NCRBUTTONDOWN | WM_NCRBUTTONUP => {}
        // SAFETY: the arguments are this message's, as Windows passed them.
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
    LRESULT(0)
}
