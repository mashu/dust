use dioxus::prelude::*;

use crate::time::sleep_ms;

const TIPS: &[&str] = &[
    "Hear the group first, then type it—let it buffer so you recognize whole letters and words.",
    "Stay relaxed; short, regular sessions work better than long cramming.",
    "If you fall behind, skip to the next group to keep rhythm and avoid pile-up.",
    "If you miss a letter, learn to let it go.",
];

#[component]
pub fn TipsCarousel() -> Element {
    let mut index = use_signal(|| 0usize);
    use_hook(|| {
        spawn(async move {
            loop {
                sleep_ms(5000).await;
                let next = (*index.peek() + 1) % TIPS.len();
                index.set(next);
            }
        });
    });
    let tip = TIPS.get(index()).copied().unwrap_or("");
    rsx! {
        div { class: "tips",
            span { class: "tips-mark", "Tip" }
            p { class: "muted", style: "margin: 0;", "{tip}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{run, Ui};

    #[test]
    fn the_carousel_moves_on_by_itself_and_comes_back_round() {
        run(|| async {
            let mut ui = Ui::new(TipsCarousel, ());
            assert!(ui.has(TIPS[0]));
            for tip in TIPS.iter().skip(1) {
                ui.advance(5_100).await;
                assert!(ui.has(tip), "expected the next tip");
            }
            // Past the last one it starts again.
            ui.advance(5_100).await;
            assert!(ui.has(TIPS[0]));
        });
    }
}
