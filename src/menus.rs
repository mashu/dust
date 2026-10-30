//! The desktop's menus: the Mac's menu bar, and a field's own menu.
//!
//! On a Mac, WKWebView hands a key it does not take itself, Cmd+V among
//! them, to the menu bar, and only an item there turns it into a paste. With
//! no menu bar nothing could be pasted, copied, cut, selected or undone in
//! any field, the word list the Words card asks you to paste among them, and
//! Cmd+Q, Cmd+H, Cmd+M and Cmd+W did nothing. Linux and Windows show no menu
//! bar: their webviews take those keys themselves.
//!
//! Dioxus also turns the page's context menu off in a release build, on
//! every desktop, as it holds Reload. A field that takes typing keeps its
//! own, with Cut, Copy and Paste.

/// One item of a menu on the Mac's menu bar. Each is one macOS knows
/// already, with the key a Mac user expects for it.
#[cfg(any(test, target_os = "macos"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MacItem {
    About,
    Hide,
    HideOthers,
    ShowAll,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Minimize,
    Close,
    Separator,
}

/// The Mac's menu bar, menu by menu. The first is the app's own, which
/// macOS shows under the app's name.
#[cfg(any(test, target_os = "macos"))]
const MAC_MENU_BAR: [(&str, &[MacItem]); 3] = {
    use MacItem::*;
    [
        (
            "Dust",
            &[About, Separator, Hide, HideOthers, ShowAll, Separator, Quit],
        ),
        (
            "Edit",
            &[Undo, Redo, Separator, Cut, Copy, Paste, SelectAll],
        ),
        ("Window", &[Minimize, Close]),
    ]
};

/// The main window's menu bar: the Mac's, and none elsewhere.
///
/// It is made before the app's event loop, so nothing in it may ask for
/// the application itself: tao has to ask first, or its own kind of
/// application is never made. That leaves out the Services menu and the
/// list of windows, which both ask at once. The items here only name what
/// they ask the application for, and ask once chosen.
#[cfg(target_os = "macos")]
pub fn menu_bar() -> Option<dioxus::desktop::muda::Menu> {
    use dioxus::desktop::muda::{Menu, Submenu};
    let bar = Menu::new();
    for (title, items) in MAC_MENU_BAR {
        let menu = Submenu::new(title, true);
        for &item in items {
            menu.append(&native(item))
                .expect("a new menu takes a new item");
        }
        bar.append(&menu).expect("a new menu bar takes a new menu");
    }
    Some(bar)
}

#[cfg(not(target_os = "macos"))]
pub fn menu_bar() -> Option<dioxus::desktop::muda::Menu> {
    None
}

#[cfg(target_os = "macos")]
fn native(item: MacItem) -> dioxus::desktop::muda::PredefinedMenuItem {
    use dioxus::desktop::muda::{AboutMetadata, PredefinedMenuItem};
    match item {
        MacItem::About => PredefinedMenuItem::about(
            Some("About Dust"),
            Some(AboutMetadata {
                name: Some("Dust".into()),
                version: Some(env!("CARGO_PKG_VERSION").into()),
                ..AboutMetadata::default()
            }),
        ),
        MacItem::Hide => PredefinedMenuItem::hide(Some("Hide Dust")),
        MacItem::HideOthers => PredefinedMenuItem::hide_others(None),
        MacItem::ShowAll => PredefinedMenuItem::show_all(None),
        MacItem::Quit => PredefinedMenuItem::quit(Some("Quit Dust")),
        MacItem::Undo => PredefinedMenuItem::undo(None),
        MacItem::Redo => PredefinedMenuItem::redo(None),
        MacItem::Cut => PredefinedMenuItem::cut(None),
        MacItem::Copy => PredefinedMenuItem::copy(None),
        MacItem::Paste => PredefinedMenuItem::paste(None),
        MacItem::SelectAll => PredefinedMenuItem::select_all(None),
        MacItem::Minimize => PredefinedMenuItem::minimize(None),
        MacItem::Close => PredefinedMenuItem::close_window(None),
        MacItem::Separator => PredefinedMenuItem::separator(),
    }
}

/// Lets a right-click in a field that takes typing open the field's own
/// menu. Dioxus turns the menu off from a listener on the document, which
/// the click, stopped on its way down, never reaches. A right-click
/// anywhere else still opens nothing.
pub const FIELD_MENU_JS: &str = r#"window.addEventListener("contextmenu", (e) => {
  const field = e.target;
  const typed = field instanceof HTMLTextAreaElement
    || (field instanceof HTMLInputElement && ["text", "number", "search"].includes(field.type));
  if (typed) e.stopPropagation();
}, true);"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// A Mac takes each of these keys to a field, or to the app, only
    /// through an item on the menu bar that asks for it: every one is
    /// there, in the menu a Mac user looks for it in.
    #[test]
    fn the_mac_menu_bar_has_an_item_for_every_key_a_mac_expects() {
        let menu = |title: &str| {
            MAC_MENU_BAR
                .iter()
                .find(|(named, _)| *named == title)
                .map(|(_, items)| *items)
                .unwrap_or_else(|| panic!("no {title} menu"))
        };
        assert_eq!(MAC_MENU_BAR[0].0, "Dust", "the app's own menu comes first");
        for item in [MacItem::About, MacItem::Hide, MacItem::Quit] {
            assert!(menu("Dust").contains(&item), "{item:?}");
        }
        for item in [
            MacItem::Undo,
            MacItem::Redo,
            MacItem::Cut,
            MacItem::Copy,
            MacItem::Paste,
            MacItem::SelectAll,
        ] {
            assert!(menu("Edit").contains(&item), "{item:?}");
        }
        for item in [MacItem::Minimize, MacItem::Close] {
            assert!(menu("Window").contains(&item), "{item:?}");
        }
    }

    /// Linux and Windows keep the window bare: their webviews take the
    /// editing keys themselves.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn only_a_mac_has_a_menu_bar() {
        assert!(menu_bar().is_none());
    }

    /// A right-click in a field that takes typing is stopped before the
    /// document's listener can turn its menu off, and only there: a
    /// button, a slider or a checkbox opens nothing, as before.
    #[test]
    fn only_a_field_that_takes_typing_keeps_its_context_menu() {
        assert!(FIELD_MENU_JS.starts_with(r#"window.addEventListener("contextmenu","#));
        assert!(
            FIELD_MENU_JS.ends_with("}, true);"),
            "heard on the way down"
        );
        assert!(FIELD_MENU_JS.contains("if (typed) e.stopPropagation();"));
        assert!(!FIELD_MENU_JS.contains("preventDefault"));
        assert!(FIELD_MENU_JS.contains("HTMLTextAreaElement"));
        for kind in ["checkbox", "range", "button", "file"] {
            assert!(!FIELD_MENU_JS.contains(kind), "{kind}");
        }
    }
}
