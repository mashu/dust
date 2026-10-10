//! Layout invariants the screens depend on. The HTML tests never paint, so
//! these assertions catch the stylesheet drifting back to rules that shove
//! the nav off-center, hide sliders, or mix theme tokens.

#[test]
fn the_stylesheet_keeps_the_layout_the_ui_needs() {
    let css = include_str!("../../assets/styles.css");

    assert!(
        css.contains(".bottom-dock"),
        "the Practice/Stats/Settings bar has to be flex-centered, not left:50%"
    );
    assert!(
        !css.contains("transform: translateX(-50%)"),
        "centering with transform used to shove the bottom nav to the right"
    );
    assert!(
        !css.contains("\n  left: 50%;"),
        "left: 50% without a matching transform is the shifted-nav bug"
    );
    assert!(
        !css.contains("color-mix("),
        "color-mix leaves header/nav unthemed when the engine does not implement it"
    );
    assert!(
        !css.contains(":root:not([data-theme"),
        "dark tokens on :root:not([data-theme=light]) paint html dark while Auto is light"
    );
    assert!(
        !css.contains(".app-root:not([data-theme"),
        "Auto must not inherit dark tokens from :not([data-theme=light])"
    );
    assert!(
        css.contains(".app-root[data-theme=\"auto\"]"),
        "Auto is an explicit theme so the OS media query has a single target"
    );
    assert!(
        css.contains(".slider-bar") && css.contains(".slider-fill") && css.contains(".slider-knob"),
        "range thumbs are webkit/moz-only; the track has to be real boxes"
    );
    assert!(
        css.contains(".switch.on i { left: 22px"),
        "the switch knob has to move with left, not transform"
    );

    let modern_rgb = regex_rgb_slash(css);
    assert!(
        modern_rgb.is_empty(),
        "CSS Color 4 rgb() with slash alpha is not portable: {modern_rgb:?}"
    );
}

fn regex_rgb_slash(css: &str) -> Vec<String> {
    let mut hits = Vec::new();
    let bytes = css.as_bytes();
    let needle = b"rgb(";
    let mut i = 0;
    while i + 4 < bytes.len() {
        if bytes[i..].starts_with(needle)
            && let Some(end) = css[i..].find(')')
        {
            let token = &css[i..i + end + 1];
            if token.contains('/') {
                hits.push(token.to_string());
            }
            i += end + 1;
            continue;
        }
        i += 1;
    }
    hits
}
