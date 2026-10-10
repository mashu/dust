//! Focus the live group answer box. Must run from a component: `document::eval`
//! without a Document context is a no-op.

pub fn focus_group_input(index: usize) {
    // Every build runs in a webview or a browser, where eval works. Tests
    // rebuild a VirtualDom with no document, so eval stays off there.
    #[cfg(all(
        any(
            feature = "web",
            feature = "desktop",
            feature = "mobile",
            feature = "mobile-silent"
        ),
        not(test)
    ))]
    {
        let js = format!(
            r#"(() => {{
            const cardId = "group-card-{index}";
            const inputId = "group-input-{index}";
            let scrolled = false;
            const apply = () => {{
                const el = document.getElementById(inputId);
                if (!el || el.disabled || el.readOnly) {{
                    return false;
                }}
                const card = document.getElementById(cardId);
                if (card && !scrolled) {{
                    // Instant, not smooth. A smooth scroll is an animation
                    // running for a few hundred milliseconds starting exactly
                    // when the next group opens and you begin typing into it,
                    // and the browser spends that time laying out rather than
                    // echoing your keys.
                    card.scrollIntoView({{ block: "center", inline: "nearest" }});
                    scrolled = true;
                }}
                if (document.activeElement !== el) {{
                    el.focus({{ preventScroll: true }});
                }}
                return document.activeElement === el;
            }};
            if (apply()) {{
                return;
            }}
            const delays = [0, 32, 80, 160];
            const tick = (i) => {{
                if (apply() || i >= delays.length) {{
                    return;
                }}
                setTimeout(() => tick(i + 1), delays[i]);
            }};
            requestAnimationFrame(() => tick(0));
        }})()"#
        );
        let _ = dioxus::document::eval(&js);
    }
    #[cfg(not(all(
        any(
            feature = "web",
            feature = "desktop",
            feature = "mobile",
            feature = "mobile-silent"
        ),
        not(test)
    )))]
    {
        let _ = index;
    }
}
