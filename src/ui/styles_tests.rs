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

const CSS: &str = include_str!("../../assets/styles.css");

/// The stylesheet with its comments taken out.
fn without_comments(css: &str) -> String {
    let mut out = String::new();
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = rest[start..]
            .find("*/")
            .map_or("", |end| &rest[start + end + 2..]);
    }
    out.push_str(rest);
    out
}

/// The declarations of the first rule whose selector is `selector`, one per
/// line, trimmed.
fn declarations(css: &str, selector: &str) -> Vec<String> {
    let start = css
        .find(&format!("{selector} {{"))
        .unwrap_or_else(|| panic!("no rule for {selector}"));
    let body = &css[start..];
    let open = body.find('{').expect("a rule opens") + 1;
    let close = open + body[open..].find('}').expect("a rule closes");
    body[open..close]
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Auto in a dark system and pinned Dark are one palette written twice,
/// because one of them has to sit inside a media query. They must not drift.
#[test]
fn both_dark_palettes_are_the_same_palette() {
    let css = without_comments(CSS);
    let auto = declarations(&css, "  .app-root[data-theme=\"auto\"]");
    let dark = declarations(&css, "\n.app-root[data-theme=\"dark\"]");
    assert!(auto.len() > 30, "the auto palette was not found whole");
    assert_eq!(auto, dark);
}

/// Every string literal in the app's sources, split into the words a `class`
/// attribute is made of.
fn source_words() -> std::collections::BTreeSet<String> {
    fn walk(dir: &std::path::Path, out: &mut String) {
        for entry in std::fs::read_dir(dir).expect("the sources can be read") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push_str(&std::fs::read_to_string(&path).expect("a source file"));
            }
        }
    }
    let mut src = String::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut src,
    );
    let mut words = std::collections::BTreeSet::new();
    let mut chars = src.chars();
    let mut previous = ' ';
    while let Some(ch) = chars.next() {
        // A `'"'` character literal opens no string.
        if ch == '"' && previous != '\'' {
            let mut literal = String::new();
            while let Some(inner) = chars.next() {
                match inner {
                    '\\' => {
                        chars.next();
                    }
                    '"' => break,
                    _ => literal.push(inner),
                }
            }
            words.extend(literal.split_whitespace().map(str::to_string));
        }
        previous = ch;
    }
    words
}

/// Every class the stylesheet styles is drawn somewhere, and every token it
/// defines is read. A rule for markup that has gone is never noticed, which
/// is how ninety lines of them, and forty-odd tokens, built up.
#[test]
fn the_stylesheet_styles_only_what_the_app_draws() {
    let css = without_comments(CSS);
    // Selectors only: take the declaration blocks out, innermost first.
    let mut selectors = css.clone();
    while let Some(close) = selectors.find('}') {
        let open = selectors[..close].rfind('{').expect("braces pair up");
        selectors.replace_range(open..=close, " ");
    }
    let words = source_words();
    let mut unused: Vec<String> = selectors
        .match_indices('.')
        .map(|(at, _)| {
            selectors[at + 1..]
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
                .collect::<String>()
        })
        .filter(|name| name.starts_with(|ch: char| ch.is_ascii_alphabetic()))
        .filter(|name| !words.contains(name))
        .collect();
    unused.sort();
    unused.dedup();
    assert!(unused.is_empty(), "styled but never drawn: {unused:?}");

    let mut unread: Vec<String> = css
        .match_indices("--")
        .map(|(at, _)| {
            css[at..]
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
                .collect::<String>()
        })
        .filter(|name| css.contains(&format!("{name}:")))
        .filter(|name| !css.contains(&format!("var({name})")))
        .collect();
    unread.sort();
    unread.dedup();
    assert!(unread.is_empty(), "defined but never read: {unread:?}");
}

/// Every rule that holds declarations, as its selector list and its
/// declarations split into property and value. At-rules such as `@media` are
/// looked through: only the rules inside them hold declarations.
fn rules(css: &str) -> Vec<(&str, Vec<(&str, &str)>)> {
    let mut rules = Vec::new();
    let mut prelude = 0;
    let mut open: Option<(&str, usize)> = None;
    for (at, ch) in css.char_indices() {
        match ch {
            '{' => {
                open = Some((css[prelude..at].trim(), at + 1));
                prelude = at + 1;
            }
            '}' => {
                // A block that opened another, like `@media`, already gave
                // its place to the rule inside it.
                if let Some((selector, body)) = open.take() {
                    let declarations = css[body..at]
                        .split(';')
                        .filter_map(|declaration| declaration.split_once(':'))
                        .map(|(property, value)| (property.trim(), value.trim()))
                        .collect();
                    rules.push((selector, declarations));
                }
                prelude = at + 1;
            }
            _ => {}
        }
    }
    rules
}

/// The selectors of a selector list, split at the commas outside brackets,
/// so `:has(a, b)` stays whole.
fn selector_list(list: &str) -> Vec<&str> {
    let mut selectors = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (at, ch) in list.char_indices() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                selectors.push(list[start..at].trim());
                start = at + 1;
            }
            _ => {}
        }
    }
    selectors.push(list[start..].trim());
    selectors
}

/// The last compound of a selector, the one that says what it picks out:
/// `body:hover` for `html > body:hover`.
fn subject(selector: &str) -> &str {
    let mut depth = 0;
    let mut start = 0;
    for (at, ch) in selector.char_indices() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '>' | '+' | '~' if depth == 0 => start = at + 1,
            _ if depth == 0 && ch.is_whitespace() => start = at + 1,
            _ => {}
        }
    }
    &selector[start..]
}

/// The element type a selector picks out, from its last compound: `body` for
/// `html > body:hover`, and nothing for `.app-root`.
fn subject_type(selector: &str) -> &str {
    let compound = subject(selector);
    let end = compound
        .find(|ch: char| !ch.is_ascii_alphanumeric())
        .unwrap_or(compound.len());
    &compound[..end]
}

/// The classes a selector's last compound asks for, leaving out those inside
/// brackets: `status-dot` and `live` for `.shell .status-dot.live:not(.x)`.
fn subject_classes(selector: &str) -> Vec<&str> {
    let compound = subject(selector);
    let mut classes = Vec::new();
    let mut depth = 0;
    for (at, ch) in compound.char_indices() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '.' if depth == 0 => {
                let name = &compound[at + 1..];
                let end = name
                    .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'))
                    .unwrap_or(name.len());
                classes.push(&name[..end]);
            }
            _ => {}
        }
    }
    classes
}

/// Whether a background value draws an image: a gradient or a picture.
fn draws_an_image(value: &str) -> bool {
    value.contains("gradient(") || value.contains("url(")
}

/// What the block opened by `prelude` holds, nested rules and all.
fn block<'a>(css: &'a str, prelude: &str) -> &'a str {
    let start = css
        .find(&format!("{prelude} {{"))
        .unwrap_or_else(|| panic!("no block for {prelude}"));
    let open = start + css[start..].find('{').expect("a block opens") + 1;
    let mut depth = 1;
    for (at, ch) in css[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &css[open..open + at];
                }
            }
            _ => {}
        }
    }
    panic!("{prelude} never closes")
}

/// .app-root covers the whole page in an opaque colour, so nothing html or
/// body paints shows on a mounted screen. Two things rely on that: body going
/// without a background, and the Linux window head letting the paper scroll
/// with the document (src/main.rs). Through a see-through .app-root that
/// paper would show stretched over the whole document on Linux, and fixed to
/// the window everywhere else. A literal colour comes first, for an engine
/// without custom properties, and the token after it is opaque in every
/// palette.
#[test]
fn the_app_root_covers_the_page_in_an_opaque_colour() {
    fn opaque_hex(colour: &str) -> bool {
        colour.strip_prefix('#').is_some_and(|hex| {
            matches!(hex.len(), 3 | 6) && hex.chars().all(|ch| ch.is_ascii_hexdigit())
        })
    }
    let css = without_comments(CSS);
    let rules = rules(&css);
    let colours: Vec<&str> = rules
        .iter()
        .filter(|(selector, _)| selector_list(selector).contains(&".app-root"))
        .flat_map(|(_, declarations)| declarations)
        .filter(|(property, _)| *property == "background-color")
        .map(|(_, value)| *value)
        .collect();
    assert_eq!(
        colours,
        ["#f2e9d9", "var(--bg)"],
        ".app-root paints the page, a literal colour first"
    );

    // No rule for .app-root, however specific, lets the page show through.
    for (selector, declarations) in rules.iter().filter(|(selector, _)| {
        selector_list(selector)
            .iter()
            .any(|one| subject_classes(one).contains(&"app-root"))
    }) {
        for (property, value) in declarations {
            assert!(
                !matches!(*property, "background" | "background-color")
                    || *value == "var(--bg)"
                    || opaque_hex(value),
                "{selector} lets the page show through .app-root: {property}: {value}"
            );
        }
    }

    let page_colours: Vec<&str> = rules
        .iter()
        .flat_map(|(_, declarations)| declarations)
        .filter(|(property, _)| *property == "--bg")
        .map(|(_, value)| *value)
        .collect();
    assert!(!page_colours.is_empty(), "the palettes' --bg was not found");
    for colour in page_colours {
        assert!(
            opaque_hex(colour),
            "a palette's --bg of {colour} is see-through"
        );
    }
}

/// Only the root paints the paper, and only it is fixed to the window. On any
/// other box a fixed background puts WebKit on its slow scroll path, where
/// every scroll frame repaints the whole window and the desktop app waits on
/// each of them. The selector is plain `html` because the Linux window head
/// overrides this rule on source order alone (src/main.rs), which a more
/// specific rule, `:root` included, would quietly defeat. So no other rule
/// for the root touches the repeat or the attachment the head overrides, and
/// no later plain `html` rule takes the paper's place. The paper the head
/// lets scroll goes unseen only while .app-root covers the page, which
/// `the_app_root_covers_the_page_in_an_opaque_colour` holds to.
#[test]
fn only_the_root_has_a_fixed_background() {
    let css = without_comments(CSS);
    let rules = rules(&css);
    let fixed: Vec<&str> = rules
        .iter()
        .filter(|(_, declarations)| {
            declarations.iter().any(|(property, value)| {
                property.starts_with("background")
                    && value
                        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-'))
                        .any(|word| word == "fixed")
            })
        })
        .map(|(selector, _)| *selector)
        .collect();
    assert_eq!(fixed, ["html"], "only the root's background is fixed");

    // No more specific rule for the root sets the repeat or the attachment.
    // The dark themes' rules set the shorthand, which resets both, but that
    // shows only under an image, and they draw none.
    for (selector, declarations) in &rules {
        let for_the_root = selector_list(selector).iter().any(|one| {
            *one != "html" && (subject_type(one) == "html" || subject(one).starts_with(":root"))
        });
        if !for_the_root {
            continue;
        }
        for (property, value) in declarations {
            assert!(
                !matches!(*property, "background-repeat" | "background-attachment")
                    && !(*property == "background" && draws_an_image(value)),
                "{selector} outranks the window head on the root: {property}: {value}"
            );
        }
    }

    // A later plain `html` rule would win on source order, and the shorthand
    // in one would wipe the paper.
    let (_, paper) = rules
        .iter()
        .rfind(|(selector, declarations)| {
            selector_list(selector).contains(&"html")
                && declarations
                    .iter()
                    .any(|(property, _)| property.starts_with("background"))
        })
        .expect("the root has a background");
    assert!(
        paper
            .iter()
            .any(|(property, value)| *property == "background-image" && draws_an_image(value)),
        "the last html rule to set a background is not the paper: {paper:?}"
    );
    assert!(
        paper.contains(&("background-attachment", "fixed")),
        "the paper is fixed to the window: {paper:?}"
    );
    let colours: Vec<&str> = paper
        .iter()
        .filter(|(property, _)| *property == "background-color")
        .map(|(_, value)| *value)
        .collect();
    assert_eq!(
        colours,
        ["#f2e9d9", "var(--bg)"],
        "a literal colour comes first, for an engine without custom properties"
    );
}

/// Whether a background declaration paints anything, or puts WebKit on its
/// slow scroll path: an image other than `none`, a colour other than
/// `transparent`, or a fixed attachment. The shorthand paints nothing only
/// when all it names is `none` and `transparent`. The other longhands only
/// place or clip what those paint.
fn paints(property: &str, value: &str) -> bool {
    let value = value.trim_end_matches("!important").trim();
    match property {
        "background" => value
            .split_whitespace()
            .any(|word| !matches!(word, "none" | "transparent")),
        "background-image" => value.split(',').any(|layer| layer.trim() != "none"),
        "background-color" => value != "transparent",
        "background-attachment" => value.split(',').any(|layer| layer.trim() == "fixed"),
        _ => false,
    }
}

/// Body paints nothing of its own, so the root alone paints the paper.
/// Body's box is covered by .app-root anyway (see
/// `the_app_root_covers_the_page_in_an_opaque_colour`); a background of its
/// own would only cost a paint, and a fixed one a slow scroll on every frame.
/// Saying outright that it has none, as `background: none` does, is no harm.
#[test]
fn the_body_paints_no_background_of_its_own() {
    // What counts as painting, so the check below cannot pass everything.
    assert!(paints("background", "var(--bg) fixed"));
    assert!(paints("background-image", "none, url(paper.png)"));
    assert!(paints("background-color", "#f2e9d9"));
    assert!(paints("background-attachment", "scroll, fixed"));
    assert!(!paints("background", "none"));
    assert!(!paints("background-color", "transparent !important"));
    assert!(!paints("background-repeat", "no-repeat"));

    let css = without_comments(CSS);
    let body_rules: Vec<_> = rules(&css)
        .into_iter()
        .filter(|(selector, _)| {
            selector_list(selector)
                .iter()
                .any(|one| subject_type(one) == "body")
        })
        .collect();
    assert!(!body_rules.is_empty(), "the rules for body were not found");
    for (selector, declarations) in body_rules {
        let painted: Vec<_> = declarations
            .iter()
            .filter(|(property, value)| paints(property, value))
            .collect();
        assert!(
            painted.is_empty(),
            "{selector} gives body a background of its own: {painted:?}"
        );
    }
}

/// The sending dot pulses for as long as a group is sent, and it grows a
/// box-shadow, which no engine composites: a smooth pulse repaints on every
/// frame. Held steps let the webview rest between them. The shorthand resets
/// the timing, so the steps have to follow it, and the shorthand's own easing
/// is what an engine without `jump-none` keeps. Every rule for the dot is
/// read, in source order, so a later one cannot put the smooth pulse back.
#[test]
fn the_sending_dot_pulses_in_held_steps() {
    let css = without_comments(CSS);
    let timing: Vec<String> = rules(&css)
        .iter()
        .filter(|(selector, _)| {
            selector_list(selector).iter().any(|one| {
                let classes = subject_classes(one);
                classes.contains(&"status-dot") && classes.contains(&"live")
            })
        })
        .flat_map(|(_, declarations)| declarations)
        .filter(|(property, _)| matches!(*property, "animation" | "animation-timing-function"))
        .map(|(property, value)| {
            format!(
                "{property}: {}",
                value.split_whitespace().collect::<Vec<_>>().join(" ")
            )
        })
        .collect();
    let [.., smooth, held] = timing.as_slice() else {
        panic!("the dot does not pulse: {timing:?}");
    };
    assert_eq!(
        held, "animation-timing-function: steps(6, jump-none)",
        "the pulse moves in held steps, and nothing after them undoes it: {timing:?}"
    );
    assert_eq!(
        smooth, "animation: pulse-dot 1.1s ease-in-out infinite",
        "the dot pulses for the whole send, smoothly where steps are unknown: {timing:?}"
    );
}

/// Reduced motion stills every animation, the sending dot's among them, on
/// the pseudo-elements as well as the elements: each runs once, and is over
/// in well under a millisecond. `*` loses to every rule that sets an
/// animation, so only `!important` reaches them.
#[test]
fn reduced_motion_stills_every_animation() {
    let css = without_comments(CSS);
    let reduced = block(&css, "@media (prefers-reduced-motion: reduce)");
    let (selector, declarations) = rules(reduced)
        .into_iter()
        .find(|(selector, _)| selector_list(selector).contains(&"*"))
        .expect("reduced motion has a rule for every element");
    for every in ["*", "*::before", "*::after"] {
        assert!(
            selector_list(selector).contains(&every),
            "reduced motion leaves {every} moving"
        );
    }
    let forced = |property: &str| {
        declarations
            .iter()
            .rfind(|(name, _)| *name == property)
            .and_then(|(_, value)| value.strip_suffix("!important"))
            .map(str::trim)
            .unwrap_or_else(|| panic!("reduced motion has to force {property}"))
    };
    let duration = forced("animation-duration");
    let ms = match duration.strip_suffix("ms") {
        Some(ms) => ms.parse::<f64>().ok(),
        None => duration
            .strip_suffix('s')
            .and_then(|s| s.parse::<f64>().ok())
            .map(|s| s * 1000.0),
    }
    .unwrap_or_else(|| panic!("an animation-duration of {duration} is no time"));
    assert!(
        (0.0..1.0).contains(&ms),
        "reduced motion still lets every animation run for {duration}"
    );
    assert_eq!(
        forced("animation-iteration-count"),
        "1",
        "reduced motion lets every animation run once"
    );
}

/// Group characters slash their zero, so it never reads as O, and nothing
/// else does: the face that draws the slash holds the one digit, and sits
/// first in the stack of exactly the rules for sent and copied characters.
/// Anywhere else it would turn every zero of a speed, a score or a date.
#[test]
fn only_group_characters_slash_their_zero() {
    let css = without_comments(CSS);
    let rules = rules(&css);
    let faces: Vec<_> = rules
        .iter()
        .filter(|(selector, _)| *selector == "@font-face")
        .map(|(_, declarations)| declarations)
        .collect();
    assert_eq!(faces.len(), 2, "the zero comes in a medium and a bold face");
    for face in faces {
        assert!(
            face.contains(&("font-family", "\"Dust Zero\"")),
            "a face that is not the zero's: {face:?}"
        );
        assert!(
            face.contains(&("unicode-range", "U+0030")),
            "the zero's face has to take the digit and nothing else: {face:?}"
        );
    }

    let stack = "\"Dust Zero\", \"IBM Plex Mono\", ui-monospace, monospace";
    let mut slashed = Vec::new();
    for (selector, declarations) in rules
        .iter()
        .filter(|(selector, _)| *selector != "@font-face")
    {
        for (property, value) in declarations {
            if *property == "font-family" && value.contains("Dust Zero") {
                assert_eq!(
                    *value, stack,
                    "{selector} puts the zero's face out of place"
                );
                slashed.push(*selector);
            }
        }
    }
    slashed.sort_unstable();
    assert_eq!(
        slashed,
        [
            ".answer",
            ".call",
            ".ch",
            ".group-text",
            ".letter-chip",
            ".listen-heard-item",
            ".log-call, .log-fields",
            ".paddle-heard",
            ".pool-chip",
            ".sent",
        ],
        "the slashed zero is for group characters only"
    );
}
