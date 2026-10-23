//! Hears a click that lands anywhere but on a widget's own squares, and each
//! press of Escape: what takes down a card a tap pinned. The practice
//! calendar and the bigram grid each listen, under a slot of their own on
//! the page, so that one going never takes the other's listener with it.

use dioxus::prelude::*;

/// Where a widget keeps its listener on the page, and what counts as its
/// own: a click on one of those is the widget's to answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Elsewhere {
    slot: &'static str,
    own: &'static str,
}

/// The practice calendar: its day squares are buttons, and the legend's
/// swatches, which share their class, are not.
pub const DAY_CARD: Elsewhere = Elsewhere {
    slot: "__dustDayCard",
    own: "button.heat-cell",
};

/// The bigram grid's pairs.
pub const PAIR_CARD: Elsewhere = Elsewhere {
    slot: "__dustPairCard",
    own: "button.bigram-cell",
};

/// The script that tells the widget of each click off its own squares, and
/// each press of Escape. A listener left there before is taken off first.
fn listen_script(at: Elsewhere) -> String {
    let Elsewhere { slot, own } = at;
    format!(
        r#"{}
const clicked = (e) => {{
    if (!(e.target instanceof Element && e.target.closest("{own}"))) {{
        dioxus.send(true);
    }}
}};
const keyed = (e) => {{
    if (e.key === "Escape") {{
        dioxus.send(true);
    }}
}};
document.addEventListener("click", clicked);
document.addEventListener("keydown", keyed);
window.{slot} = () => {{
    document.removeEventListener("click", clicked);
    document.removeEventListener("keydown", keyed);
}};"#,
        stop_script(at)
    )
}

/// The script that takes the listener off again.
fn stop_script(at: Elsewhere) -> String {
    let slot = at.slot;
    format!(
        r#"if (window.{slot}) {{
    window.{slot}();
    window.{slot} = null;
}}"#
    )
}

/// While the calling component is mounted, `on_elsewhere` hears each click
/// that lands anywhere but on `at`'s own squares, and each press of Escape.
pub fn use_click_elsewhere(at: Elsewhere, mut on_elsewhere: impl FnMut() + 'static) {
    let on_elsewhere = use_callback(move |()| on_elsewhere());
    // Tests have no page to run it on: they click elsewhere through
    // [`crate::testing::Ui::click_elsewhere`].
    #[cfg(not(test))]
    {
        use_hook(move || {
            spawn(async move {
                let mut page = document::eval(&listen_script(at));
                while page.recv::<bool>().await.is_ok() {
                    on_elsewhere.call(());
                }
            })
        });
        use_drop(move || {
            dioxus::core::spawn_forever(async move {
                let _ = document::eval(&stop_script(at)).await;
            });
        });
    }
    #[cfg(test)]
    use_hook(move || {
        let _ = at;
        crate::testing::listen_elsewhere(on_elsewhere);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The harness never mounts anything on a page, so the script is the
    /// part a test can see: a click off the widget's squares tells it, as
    /// Escape does, a click on one is the square's own, and the listener
    /// comes off again, one left before first. Each widget's is its own.
    #[test]
    fn the_page_says_when_it_is_clicked_off_the_squares() {
        for (at, own, other) in [
            (DAY_CARD, "button.heat-cell", PAIR_CARD),
            (PAIR_CARD, "button.bigram-cell", DAY_CARD),
        ] {
            let script = listen_script(at);
            assert!(
                script.contains(&format!(r#"e.target.closest("{own}")"#)),
                "{script}"
            );
            assert!(script.contains(r#"e.key === "Escape""#), "{script}");
            assert_eq!(script.matches("dioxus.send(true)").count(), 2, "{script}");
            for (on, off) in [
                (
                    r#"addEventListener("click", clicked)"#,
                    r#"removeEventListener("click", clicked)"#,
                ),
                (
                    r#"addEventListener("keydown", keyed)"#,
                    r#"removeEventListener("keydown", keyed)"#,
                ),
            ] {
                assert!(script.contains(on), "{script}");
                assert!(script.contains(off), "{script}");
            }
            let off = script
                .find(&format!("window.{}();", at.slot))
                .expect("taken off");
            let on = script.find("addEventListener").expect("put on");
            assert!(off < on, "{script}");
            assert!(stop_script(at).contains(&format!("window.{} = null;", at.slot)));
            assert!(!script.contains(other.slot), "{script}");
        }
    }
}
