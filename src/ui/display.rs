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
    /// The graphics card, by name, handing each frame to the window.
    Card(String),
    /// The graphics card, by name, each of its frames copied to the window
    /// by the processor, for the reason given. Only WebKitGTK's own page
    /// says so.
    #[cfg_attr(
        not(any(test, all(feature = "desktop", target_os = "linux"))),
        allow(dead_code)
    )]
    CardCopied { name: String, why: String },
    /// The processor paints the page, and the graphics card, by name, only
    /// puts its tiles together, for the reason given if there is one.
    #[cfg_attr(
        not(any(test, all(feature = "desktop", target_os = "linux"))),
        allow(dead_code)
    )]
    PaintedByProcessor { card: String, why: Option<String> },
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
        Drawn::CardCopied { name, why } => format!(
            "Drawn on the graphics card ({name}), but each frame is copied to the window by the processor, which costs it a little on every scroll: {why}."
        ),
        Drawn::PaintedByProcessor { card, why } => format!(
            "Painted by the processor, and only put together on the graphics card ({card}): scrolling and animation cost the processor, and can lag{}.",
            why.as_ref()
                .map_or(String::new(), |why| format!(", as {why}"))
        ),
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

/// WebKitGTK's own word on how it draws, from its `webkit://gpu` page.
#[cfg(any(test, all(feature = "desktop", target_os = "linux")))]
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GpuPage {
    /// Its hardware acceleration: "always", or "never".
    policy: Option<String>,
    /// The GL renderer it composites with.
    gl_renderer: Option<String>,
    /// How its frames reach the window: "DMABuf (Supported buffers:
    /// Hardware, Shared Memory)", say.
    renderer: Option<String>,
    /// What the window takes them through: none under X11.
    native_interface: Option<String>,
    /// What paints the page's tiles: "GPU (2 threads)", "CPU (4 threads)",
    /// or "Disabled" with no painting threads. WebKitGTK says so from 2.52.
    threaded: Option<String>,
}

/// Whether an environment variable WebKitGTK reads is on: set, and not to
/// "0", as WebKitGTK takes it.
#[cfg(any(test, all(feature = "desktop", target_os = "linux")))]
fn on(env: &impl Fn(&str) -> Option<String>, name: &str) -> bool {
    env(name).is_some_and(|value| value != "0")
}

/// What WebKitGTK's page says, read with the environment `env` it runs in,
/// and what turned its DMA-BUF renderer off, if anything did.
#[cfg(any(test, all(feature = "desktop", target_os = "linux")))]
fn classify(
    page: &GpuPage,
    env: impl Fn(&str) -> Option<String>,
    dmabuf_off: Option<&str>,
) -> Looked {
    let never = page
        .policy
        .as_deref()
        .is_some_and(|policy| policy.eq_ignore_ascii_case("never"));
    let compositing_off = if on(&env, "WEBKIT_DISABLE_COMPOSITING_MODE") {
        Some("WEBKIT_DISABLE_COMPOSITING_MODE is set".to_string())
    } else if never {
        // What WebKitGTK falls back to never under, or failing that, its
        // own finding: no graphics driver it can use.
        Some(
            dmabuf_off
                .unwrap_or("WebKitGTK's hardware acceleration is set to never, as it is when it finds no graphics driver it can use")
                .to_string(),
        )
    } else {
        None
    };
    // Skia paints on the processor where the page says so; with no word
    // from it (before 2.52, or no painting threads), where the variable
    // WebKitGTK reads asks it to.
    let cpu_asked = on(&env, "WEBKIT_SKIA_ENABLE_CPU_RENDERING");
    let painted_by_processor = match page.threaded.as_deref().map(str::to_ascii_uppercase) {
        Some(threads) if threads.starts_with("CPU") => true,
        Some(threads) if threads.starts_with("GPU") => false,
        _ => cpu_asked,
    };
    // Only the DMA-BUF renderer hands frames over in buffers: WebKitGTK's
    // older backing stores, "XWindow" under X11 and "WPE", say no more than
    // their name, and copy nothing through the processor.
    let dmabuf = page
        .renderer
        .as_deref()
        .is_some_and(|renderer| renderer.starts_with("DMABuf"));
    let drawn = match drawn_by(page.gl_renderer.clone()) {
        Drawn::Card(card) if painted_by_processor => Drawn::PaintedByProcessor {
            card,
            why: cpu_asked.then(|| "WEBKIT_SKIA_ENABLE_CPU_RENDERING asks".to_string()),
        },
        Drawn::Card(name)
            if dmabuf
                && page
                    .renderer
                    .as_deref()
                    .is_some_and(|renderer| !renderer.contains("Hardware")) =>
        {
            let why = if on(&env, "WEBKIT_DMABUF_RENDERER_FORCE_SHM") {
                "WEBKIT_DMABUF_RENDERER_FORCE_SHM is set"
            } else {
                "the graphics driver shares no buffer with the window"
            };
            Drawn::CardCopied {
                name,
                why: why.to_string(),
            }
        }
        Drawn::Card(name)
            if dmabuf
                && page
                    .native_interface
                    .as_deref()
                    .is_some_and(|interface| interface.eq_ignore_ascii_case("none")) =>
        {
            let wayland = env("XDG_SESSION_TYPE").is_some_and(|kind| kind == "wayland");
            let why = if wayland {
                "the window runs under X11 (XWayland), which takes no buffer from the card; start Dust with GDK_BACKEND=wayland to hand them over"
            } else {
                "the window, under X11, takes no buffer from the card"
            };
            Drawn::CardCopied {
                name,
                why: why.to_string(),
            }
        }
        drawn => drawn,
    };
    Looked {
        drawn,
        compositing_off,
    }
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

    use super::{GpuPage, Looked, classify};

    /// Its rows of interest as JSON, written into the page's title once the
    /// page has filled them in: the title is the one thing of a page a view
    /// hands back without a JavaScript engine of the app's own. The page
    /// fills its rows in a moment after it loads; with its acceleration
    /// never it has no GL rows to wait for.
    const READ_ROWS: &str = r#"(() => {
    const started = Date.now();
    const read = () => {
        const rows = {};
        for (const tr of document.querySelectorAll("tr")) {
            const cells = [...tr.children].map((cell) => cell.innerText.trim());
            if (cells.length >= 2) rows[cells[0]] = cells[1];
        }
        const never = (rows.Policy ?? "").toLowerCase() === "never";
        if (rows.GL_RENDERER || (rows.Policy && never) || Date.now() - started > 3000) {
            document.title = JSON.stringify({
                policy: rows.Policy ?? null,
                glRenderer: rows.GL_RENDERER ?? null,
                renderer: rows.Renderer ?? null,
                nativeInterface: rows["Native interface"] ?? null,
                threaded: rows["Threaded rendering"] ?? null,
            });
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
        let said = tokio::time::timeout(Duration::from_secs(6), said).await;
        // The view, and its page, go once they have said.
        drop(view);
        let page: GpuPage = said
            .ok()
            .and_then(Result::ok)
            .and_then(|title| serde_json::from_str(&title).ok())
            .unwrap_or_default();
        classify(
            &page,
            |name| std::env::var(name).ok(),
            crate::linux_webview::dmabuf_off(),
        )
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

    /// The environment `set` gives, and nothing else.
    fn env(set: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let set: Vec<(String, String)> = set
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        move |name| {
            set.iter()
                .find(|(set, _)| set == name)
                .map(|(_, value)| value.clone())
        }
    }

    const INTEL: &str = "Mesa Intel(R) UHD Graphics 620 (KBL GT2)";

    fn page(gl: &str) -> GpuPage {
        GpuPage {
            policy: Some("always".into()),
            gl_renderer: Some(gl.into()),
            renderer: Some("DMABuf (Supported buffers: Hardware, Shared Memory)".into()),
            native_interface: Some("Wayland".into()),
            threaded: Some("GPU (2 threads)".into()),
        }
    }

    /// WebKitGTK's page read row by row: the card handing its frames over,
    /// the card with its frames copied by the processor, the processor
    /// painting, and compositing off, each said with what is to blame.
    #[test]
    fn webkitgtk_s_page_says_what_draws_and_what_copies() {
        let clean = classify(&page(INTEL), env(&[]), None);
        assert_eq!(clean.drawn, Drawn::Card(INTEL.into()));
        assert_eq!(clean.compositing_off, None);

        let shm = GpuPage {
            renderer: Some("DMABuf (Supported buffers: Shared Memory)".into()),
            ..page(INTEL)
        };
        let copied = classify(
            &shm,
            env(&[("WEBKIT_DMABUF_RENDERER_FORCE_SHM", "1")]),
            None,
        );
        assert!(
            matches!(&copied.drawn, Drawn::CardCopied { why, .. } if why.contains("FORCE_SHM")),
            "{copied:?}"
        );

        let x11 = GpuPage {
            native_interface: Some("None".into()),
            ..page(INTEL)
        };
        let xwayland = classify(&x11, env(&[("XDG_SESSION_TYPE", "wayland")]), None);
        assert!(
            matches!(&xwayland.drawn, Drawn::CardCopied { why, .. } if why.contains("GDK_BACKEND=wayland")),
            "{xwayland:?}"
        );

        let cpu = GpuPage {
            threaded: Some("CPU (4 threads)".into()),
            ..page(INTEL)
        };
        let skia = classify(
            &cpu,
            env(&[("WEBKIT_SKIA_ENABLE_CPU_RENDERING", "1")]),
            None,
        );
        assert_eq!(
            skia.drawn,
            Drawn::PaintedByProcessor {
                card: INTEL.into(),
                why: Some("WEBKIT_SKIA_ENABLE_CPU_RENDERING asks".into()),
            }
        );
        let said = drawn_text(&skia);
        assert!(
            said.starts_with(
                "Painted by the processor, and only put together on the graphics card (Mesa Intel"
            ) && said.ends_with("can lag, as WEBKIT_SKIA_ENABLE_CPU_RENDERING asks."),
            "{said}"
        );
        // Before 2.52 the page has no word on it, and with no painting
        // threads it says "Disabled": the variable says it then.
        for threaded in [None, Some("Disabled".to_string())] {
            let quiet = GpuPage {
                threaded: threaded.clone(),
                ..page(INTEL)
            };
            let asked = classify(
                &quiet,
                env(&[("WEBKIT_SKIA_ENABLE_CPU_RENDERING", "1")]),
                None,
            );
            assert!(
                matches!(asked.drawn, Drawn::PaintedByProcessor { .. }),
                "{threaded:?}: {asked:?}"
            );
            assert_eq!(
                classify(&quiet, env(&[]), None).drawn,
                Drawn::Card(INTEL.into()),
                "{threaded:?}"
            );
        }
        // The page's own word wins over the variable.
        assert_eq!(
            classify(
                &page(INTEL),
                env(&[("WEBKIT_SKIA_ENABLE_CPU_RENDERING", "1")]),
                None
            )
            .drawn,
            Drawn::Card(INTEL.into())
        );

        // WebKitGTK 2.42's older backing stores, with the DMA-BUF renderer
        // off: X11's XComposite and WPE copy nothing through the processor,
        // and under X11 there is no native interface to name.
        for renderer in ["XWindow", "WPE"] {
            let older = GpuPage {
                renderer: Some(renderer.into()),
                native_interface: Some("None".into()),
                threaded: None,
                ..page(INTEL)
            };
            assert_eq!(
                classify(&older, env(&[("XDG_SESSION_TYPE", "wayland")]), None).drawn,
                Drawn::Card(INTEL.into()),
                "{renderer}"
            );
        }

        let soft = classify(&page("llvmpipe (LLVM 20.1.2, 256 bits)"), env(&[]), None);
        assert!(matches!(soft.drawn, Drawn::Processor(_)), "{soft:?}");

        // Compositing off: by the environment, by the renderer turned off,
        // or by WebKitGTK finding no driver; and a variable set to "0" is off.
        let never = GpuPage {
            policy: Some("never".into()),
            ..GpuPage::default()
        };
        let named = classify(
            &never,
            env(&[]),
            Some("WEBKIT_DISABLE_DMABUF_RENDERER is set"),
        );
        assert_eq!(named.drawn, Drawn::Unknown);
        assert_eq!(
            named.compositing_off.as_deref(),
            Some("WEBKIT_DISABLE_DMABUF_RENDERER is set")
        );
        assert!(
            classify(&never, env(&[]), None)
                .compositing_off
                .is_some_and(|why| why.contains("no graphics driver"))
        );
        let zero = classify(
            &page(INTEL),
            env(&[("WEBKIT_DISABLE_COMPOSITING_MODE", "0")]),
            None,
        );
        assert_eq!(zero.compositing_off, None, "set to 0 is not set");
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
