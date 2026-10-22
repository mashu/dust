use dioxus::prelude::*;

use crate::time::sleep_ms;

/// Tips for training on groups.
pub const TIPS: &[&str] = &[
    "Hear the group first, then type it—let it buffer so you recognize whole letters and words.",
    "Stay relaxed; short, regular sessions work better than long cramming.",
    "If you fall behind, press Next to move on to the next group, keep your rhythm and avoid pile-up.",
    "If you miss a letter, learn to let it go.",
];

/// Tips for a contest run, which has no groups to move on from.
pub const CONTEST_TIPS: &[&str] = &[
    "Enter sends what the QSO needs next: CQ, then the call you typed with your exchange, then TU as it logs.",
    "Not sure of the call? Type the part you have with a ? for the rest, K5?, and Enter asks for it again.",
    "Missed the number? Enter sends ? while the exchange is empty, and the station sends it again.",
    "Stay relaxed; short, regular sessions work better than long cramming.",
];

#[component]
pub fn TipsCarousel(tips: &'static [&'static str]) -> Element {
    let mut index = use_signal(|| 0usize);
    use_hook(|| {
        spawn(async move {
            loop {
                sleep_ms(5000).await;
                let next = *index.peek() + 1;
                index.set(next);
            }
        });
    });
    // Past the last one it starts again.
    let tip = tips.get(index() % tips.len().max(1)).copied().unwrap_or("");
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
    use crate::testing::{Ui, run};

    #[test]
    fn the_carousel_moves_on_by_itself_and_comes_back_round() {
        run(|| async {
            let mut ui = Ui::new(TipsCarousel, TipsCarouselProps { tips: TIPS });
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
