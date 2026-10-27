//! WebKitGTK, the desktop app's webview on Linux, set up before it starts:
//! under the window manager's title bar, and drawing on the graphics card.
//!
//! Dioxus, left to itself, turns WebKitGTK's DMA-BUF renderer off on every
//! Wayland session with a GPU (`WEBKIT_DISABLE_DMABUF_RENDERER`), over
//! whatever was set: a workaround for NVIDIA's driver. A recent WebKitGTK,
//! 2.52 among them, then has no renderer to use the graphics card with,
//! sets its hardware acceleration to never, and paints every page by the
//! processor, so every scroll lags. Here it is kept on, except under
//! NVIDIA's own driver on a Wayland session, where its windows can come up
//! blank with it on. Either way, a value set before Dust starts is left as
//! it is.

use std::sync::atomic::{AtomicBool, Ordering};

const DMABUF: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";

/// Dust turned the DMA-BUF renderer off itself, for NVIDIA's driver.
static OFF_FOR_NVIDIA: AtomicBool = AtomicBool::new(false);

/// Set WebKitGTK up, before it starts.
///
/// # Safety
///
/// At process start, before any other thread exists: it sets the
/// environment.
pub unsafe fn prepare() {
    // GTK client-side decorations draw a thick header with the window
    // title: the window manager's own title bar instead, under X11 (on a
    // Wayland session, XWayland), as Dioxus has it, unless asked otherwise.
    unsafe {
        std::env::set_var("GTK_CSD", "0");
        if std::env::var_os("GDK_BACKEND").is_none() {
            std::env::set_var("GDK_BACKEND", "x11");
        }
    }
    let wayland = std::env::var("XDG_SESSION_TYPE").is_ok_and(|kind| kind == "wayland");
    let nvidia = std::path::Path::new("/proc/driver/nvidia/version").exists();
    if turn_dmabuf_off(wayland, nvidia, std::env::var_os(DMABUF).is_some()) {
        unsafe {
            std::env::set_var(DMABUF, "1");
        }
        OFF_FOR_NVIDIA.store(true, Ordering::Relaxed);
    }
}

/// Whether Dust turns the DMA-BUF renderer off: under NVIDIA's own driver
/// on a Wayland session, unless it was set already.
fn turn_dmabuf_off(wayland: bool, nvidia: bool, set_already: bool) -> bool {
    wayland && nvidia && !set_already
}

/// Why the DMA-BUF renderer is off, when it is: then WebKitGTK paints by
/// the processor.
#[cfg_attr(test, allow(dead_code))]
pub fn dmabuf_off() -> Option<&'static str> {
    if OFF_FOR_NVIDIA.load(Ordering::Relaxed) {
        return Some(
            "Dust turns WebKitGTK's DMA-BUF renderer off under NVIDIA's driver on Wayland, where its windows can come up blank; start Dust with WEBKIT_DISABLE_DMABUF_RENDERER=0 to try the graphics card",
        );
    }
    std::env::var(DMABUF)
        .is_ok_and(|value| value != "0")
        .then_some("WEBKIT_DISABLE_DMABUF_RENDERER is set")
}

#[cfg(test)]
mod tests {
    use super::turn_dmabuf_off;

    /// Off only under NVIDIA's own driver on Wayland, and never over a
    /// value set already.
    #[test]
    fn the_dmabuf_renderer_is_kept_on_but_for_nvidia_on_wayland() {
        assert!(turn_dmabuf_off(true, true, false));
        assert!(!turn_dmabuf_off(true, false, false), "a Wayland session");
        assert!(!turn_dmabuf_off(false, true, false), "NVIDIA under X11");
        assert!(!turn_dmabuf_off(false, false, false));
        assert!(!turn_dmabuf_off(true, true, true), "set before Dust starts");
    }
}
