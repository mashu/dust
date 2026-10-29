//! The desktop's own menu for a window, opened without a title bar.
//!
//! A window's menu — Always on Top, Move, Close and the rest — belongs to the
//! desktop, and a title bar is only one way in. GTK can ask for it from
//! anywhere in the window: on Wayland that is `xdg_toplevel.show_window_menu`,
//! the request a client makes to have the compositor show the menu its title
//! bar would, and on X11 the `_GTK_SHOW_WINDOW_MENU` message GNOME and KDE
//! both answer. The desktop then keeps the window wherever the user puts it.
//!
//! Elsewhere there is no such menu to ask for, so the window keeps itself on
//! top, which Windows and macOS let an app do.

use dioxus::desktop::DesktopService;

/// Where the pointer was pressed: in the window, and on the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Press {
    pub x: f64,
    pub y: f64,
    pub x_root: f64,
    pub y_root: f64,
}

/// Open the desktop's menu for `window` at `press`, the right-click that
/// asked for it. Returns whether the desktop took the request.
#[cfg(target_os = "linux")]
pub fn show(window: &DesktopService, press: Press) -> bool {
    use dioxus::desktop::tao::platform::unix::WindowExtUnix;
    use gtk::gdk;
    use gtk::glib::translate::{IntoGlibPtr, ToGlibPtrMut};
    use gtk::prelude::*;

    let Some(surface) = window.window.gtk_window().window() else {
        return false;
    };
    let Some(pointer) = surface
        .display()
        .default_seat()
        .and_then(|seat| seat.pointer())
    else {
        return false;
    };
    // The click reached the page, not GTK, so GTK is handed one like it. On
    // Wayland the compositor checks the request against the press it saw,
    // which GTK recorded as it went by on its way to the page.
    let mut event = gdk::Event::new(gdk::EventType::ButtonPress);
    event.set_device(Some(&pointer));
    // Safety: a button-press event is a `GdkEventButton`, and the window
    // reference handed to it is the event's own, released when it is.
    unsafe {
        let button = ToGlibPtrMut::<*mut gdk::ffi::GdkEvent>::to_glib_none_mut(&mut event).0
            as *mut gdk::ffi::GdkEventButton;
        (*button).window = surface.clone().into_glib_ptr();
        (*button).time = gtk::current_event_time();
        (*button).x = press.x;
        (*button).y = press.y;
        (*button).x_root = press.x_root;
        (*button).y_root = press.y_root;
        (*button).button = 3;
    }
    surface.show_window_menu(&mut event)
}

/// Elsewhere a right-click turns always on top over, from where the window
/// says it is: a flag of the app's own outlived the window it was about, so
/// a floating window opened again, never on top, took its first
/// right-click to be taken off the top, and nothing seemed to happen.
#[cfg(not(target_os = "linux"))]
pub fn show(window: &DesktopService, _press: Press) -> bool {
    let on_top = window.window.is_always_on_top();
    window.window.set_always_on_top(!on_top);
    true
}
