//! How this device draws Dust: on its graphics card, or by its processor.
//! Drawn by the processor, every scroll and every animation is painted by
//! it, and can lag. A webview falls back to that by itself when it cannot
//! use the graphics driver, and says nothing; this card says it.
//!
//! WebKitGTK, the desktop app's webview on Linux, tells a page's WebGL only
//! a made-up name, so there it is asked itself: its own page on its
//! graphics, `webkit://gpu`, read in a view of its own that is let go once
//! it has said. Anywhere else, WebGL's renderer is the webview's own. Either
//! is looked at only when asked: neither is made for nothing.

use dioxus::prelude::*;

use crate::ui::widgets::{CardHead, IconName, control_id, html_bool};

/// What draws the page.
#[derive(Clone, Debug, PartialEq)]
enum Drawn {
    /// The graphics card, by name.
    Card(String),
    /// The processor, by the name of the software that does it.
    Processor(String),
    /// The webview gives no word on it.
    Unknown,
}

/// What was found, and anything that turns the webview's accelerated
/// compositing off, which leaves the page to the processor whatever draws
/// the rest.
#[derive(Clone, Debug, PartialEq)]
struct Looked {
    drawn: Drawn,
    compositing_off: Option<String>,
}

fn drawn_by(renderer: Option<String>) -> Drawn {
    match renderer {
        Some(name) if software(&name) => Drawn::Processor(name),
        Some(name) => Drawn::Card(name),
        None => Drawn::Unknown,
    }
}

/// A renderer that draws in software: Mesa's llvmpipe, softpipe and
/// swrast, SwiftShader, the Microsoft Basic Render Driver.
fn software(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "llvmpipe",
        "softpipe",
        "swrast",
        "swiftshader",
        "software",
        "basic render",
    ]
    .iter()
    .any(|soft| name.contains(soft))
}

/// What draws the page, in words.
fn drawn_text(looked: &Looked) -> String {
    let mut text = match &looked.drawn {
        Drawn::Card(name) => format!("Drawn on the graphics card: {name}."),
        Drawn::Processor(name) => format!(
            "Drawn by the processor ({name}), with no graphics card: scrolling and animation cost the processor, and can lag. The webview falls back to this when it cannot use the graphics driver."
        ),
        Drawn::Unknown => "This webview does not say how it draws the page.".to_string(),
    };
    if let Some(why) = &looked.compositing_off {
        text.push_str(&format!(
            " Accelerated compositing is off, so the page is painted by the processor: {why}."
        ));
    }
    text
}

/// Ask the webview.
#[cfg(test)]
async fn look() -> Looked {
    // Tests have no webview to ask.
    Looked {
        drawn: Drawn::Unknown,
        compositing_off: None,
    }
}

#[cfg(all(not(test), feature = "desktop", target_os = "linux"))]
use webkitgtk::look;

#[cfg(all(not(test), not(all(feature = "desktop", target_os = "linux"))))]
use webgl::look;

/// WebKitGTK's own page on its graphics, read.
#[cfg(all(not(test), feature = "desktop", target_os = "linux"))]
mod webkitgtk {
    use std::cell::RefCell;
    use std::time::Duration;

    use webkit2gtk::{LoadEvent, WebViewExt};

    use super::{Looked, drawn_by};

    /// Its rows of interest as JSON, written into the page's title once the
    /// page has filled them in: the title is the one thing of a page a view
    /// hands back without a JavaScript engine of the app's own.
    const READ_ROWS: &str = r#"(() => {
    const started = Date.now();
    const read = () => {
        const rows = {};
        for (const tr of document.querySelectorAll("tr")) {
            const cells = [...tr.children].map((cell) => cell.innerText.trim());
            if (cells.length >= 2) rows[cells[0]] = cells[1];
        }
        if (rows.GL_RENDERER || Date.now() - started > 3000) {
            document.title = JSON.stringify({ policy: rows.Policy ?? null, renderer: rows.GL_RENDERER ?? null });
        } else {
            setTimeout(read, 100);
        }
    };
    read();
})();"#;

    pub async fn look() -> Looked {
        let (sent, said) = tokio::sync::oneshot::channel::<String>();
        let sent = RefCell::new(Some(sent));
        let view = webkit2gtk::WebView::new();
        view.connect_title_notify(move |view| {
            let Some(title) = view.title().filter(|title| title.starts_with('{')) else {
                return;
            };
            if let Some(sent) = sent.borrow_mut().take() {
                let _ = sent.send(title.to_string());
            }
        });
        view.connect_load_changed(|view, event| {
            if event == LoadEvent::Finished {
                view.evaluate_javascript(
                    READ_ROWS,
                    None,
                    None,
                    None::<&webkit2gtk::gio::Cancellable>,
                    |_| {},
                );
            }
        });
        view.load_uri("webkit://gpu");
        let rows = tokio::time::timeout(Duration::from_secs(6), said).await;
        // The view, and its page, go once they have said.
        drop(view);
        let rows: serde_json::Value = rows
            .ok()
            .and_then(Result::ok)
            .and_then(|title| serde_json::from_str(&title).ok())
            .unwrap_or_default();
        let field = |name: &str| {
            rows.get(name)
                .and_then(|value| value.as_str())
                .map(str::to_string)
        };
        let never = field("policy").is_some_and(|policy| policy.eq_ignore_ascii_case("never"));
        let compositing_off = if std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_some() {
            Some("WEBKIT_DISABLE_COMPOSITING_MODE is set")
        } else if never {
            // What WebKitGTK falls back to never under, or failing that,
            // its own finding: no graphics driver it can use.
            Some(crate::linux_webview::dmabuf_off().unwrap_or(
                "WebKitGTK's hardware acceleration is set to never, as it is when it finds no graphics driver it can use",
            ))
        } else {
            None
        }
        .map(str::to_string);
        Looked {
            drawn: drawn_by(field("renderer")),
            compositing_off,
        }
    }
}

/// WebGL's renderer, the webview's own.
#[cfg(all(not(test), not(all(feature = "desktop", target_os = "linux"))))]
mod webgl {
    use dioxus::prelude::*;

    use super::{Looked, drawn_by};

    /// The renderer's name, or null with no WebGL, or with only a made-up
    /// name to give: WebKit names every GPU "Apple GPU", even off Apple's
    /// devices, and a browser that resists fingerprinting names none. The
    /// context is let go at once.
    const RENDERER: &str = r#"const canvas = document.createElement("canvas");
const gl = canvas.getContext("webgl") || canvas.getContext("experimental-webgl");
if (!gl) {
    return null;
}
const info = gl.getExtension("WEBGL_debug_renderer_info");
const name = String(gl.getParameter(info ? info.UNMASKED_RENDERER_WEBGL : gl.RENDERER));
const lose = gl.getExtension("WEBGL_lose_context");
if (lose) {
    lose.loseContext();
}
const apple = /Mac|iPhone|iPad/.test(navigator.userAgent);
if ((name === "Apple GPU" && !apple) || /^(Mozilla|WebKit|WebKit WebGL|Generic Renderer)$/.test(name)) {
    return null;
}
return name;"#;

    pub async fn look() -> Looked {
        let renderer = document::eval(RENDERER)
            .await
            .ok()
            .and_then(|name| name.as_str().map(str::to_string));
        Looked {
            drawn: drawn_by(renderer),
            compositing_off: None,
        }
    }
}

#[component]
pub fn DisplayCard() -> Element {
    let mut looked = use_signal(|| None::<Looked>);
    let mut looking = use_signal(|| false);
    let check = move |_| {
        if looking() {
            return;
        }
        looking.set(true);
        spawn(async move {
            looked.set(Some(look().await));
            looking.set(false);
        });
    };
    rsx! {
        div { class: "card stack-sm",
            CardHead {
                icon: IconName::Info,
                title: "Display",
                note: "How this device draws Dust",
            }
            p { id: "display-drawn", class: "muted", style: "margin: 0;",
                if let Some(looked) = looked() {
                    "{drawn_text(&looked)}"
                } else if looking() {
                    "Looking…"
                } else {
                    "Whether the graphics card draws Dust, or the processor does, which can make scrolling lag."
                }
            }
            button {
                id: control_id("btn", "check display"),
                class: "btn btn-secondary btn-sm",
                disabled: html_bool(looking()),
                onclick: check,
                "Check"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Ui;

    #[test]
    fn software_renderers_are_told_from_graphics_cards() {
        for soft in [
            "llvmpipe (LLVM 20.1.2, 256 bits)",
            "softpipe",
            "Mesa swrast",
            "ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (Subzero)), SwiftShader driver)",
            "Microsoft Basic Render Driver",
        ] {
            assert!(
                matches!(drawn_by(Some(soft.into())), Drawn::Processor(_)),
                "{soft}"
            );
        }
        for card in [
            "Mesa Intel(R) UHD Graphics 620 (KBL GT2)",
            "NVIDIA GeForce RTX 3060/PCIe/SSE2",
            "AMD Radeon RX 6600 (radeonsi, navi23, LLVM 17.0.6, DRM 3.54)",
            "Apple M2",
            "Adreno (TM) 730",
        ] {
            assert!(
                matches!(drawn_by(Some(card.into())), Drawn::Card(_)),
                "{card}"
            );
        }
        assert_eq!(drawn_by(None), Drawn::Unknown);
    }

    #[test]
    fn the_card_says_what_draws_the_page_and_why_it_may_lag() {
        let card = Looked {
            drawn: Drawn::Card("Apple M2".into()),
            compositing_off: None,
        };
        assert_eq!(drawn_text(&card), "Drawn on the graphics card: Apple M2.");
        let soft = drawn_text(&Looked {
            drawn: Drawn::Processor("llvmpipe".into()),
            compositing_off: None,
        });
        assert!(
            soft.starts_with("Drawn by the processor (llvmpipe)"),
            "{soft}"
        );
        assert!(soft.contains("can lag"), "{soft}");
        let off = drawn_text(&Looked {
            compositing_off: Some("WEBKIT_DISABLE_COMPOSITING_MODE is set".into()),
            ..card
        });
        assert!(
            off.ends_with("painted by the processor: WEBKIT_DISABLE_COMPOSITING_MODE is set."),
            "{off}"
        );
    }

    /// Nothing is asked of the webview until Check is pressed.
    #[test]
    fn the_webview_is_asked_only_when_check_is_pressed() {
        crate::testing::run(|| async {
            let mut ui = Ui::new(DisplayCard, ());
            assert!(
                ui.has("Whether the graphics card draws Dust"),
                "{}",
                ui.html()
            );
            ui.click("btn-check-display");
            ui.advance(10).await;
            assert!(ui.has("This webview does not say how it draws the page."));
        });
    }
}
