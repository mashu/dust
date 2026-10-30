use dioxus::prelude::*;

use crate::time::sleep_ms;

/// Tips for training on groups.
pub const TIPS: &[&str] = &[
    "Type each character the moment you hear it—that is how Stats → Letters times how fast you recognise it.",
    "Stay relaxed; short, regular sessions work better than long cramming.",
    "If you fall behind, press Next to move on to the next group, keep your rhythm and avoid pile-up.",
    "If you miss a letter, learn to let it go.",
];

/// Tips for a contest run, which has no groups to move on from.
pub const CONTEST_TIPS: &[&str] = &[
    "Enter sends what the QSO needs next: CQ, then the call you typed with your exchange, then TU as it logs.",
    "Not sure of the call? Type the part you have with a ? for the rest, K5?, and Enter asks for it again. Where ? is on a key right of P, that key is a paddle: turn off Paddles on the keys right of P to type it.",
    "Missed the number? Enter sends ? while the exchange is empty, and the station sends it again.",
    "Stay relaxed; short, regular sessions work better than long cramming.",
];

/// How long each tip stays up before the next: seven seconds, time to read
/// one that wraps to a few lines on a phone.
const TIP_MS: u32 = 7_000;

#[component]
pub fn TipsCarousel(tips: &'static [&'static str]) -> Element {
    let mut index = use_signal(|| 0usize);
    use_hook(|| {
        spawn(async move {
            loop {
                sleep_ms(TIP_MS).await;
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
            p { class: "muted hint", "{tip}" }
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
            // Each one stays up for seven seconds.
            ui.advance(6_900).await;
            assert!(ui.has(TIPS[0]), "moved on too soon");
            ui.advance(200).await;
            assert!(ui.has(TIPS[1]), "expected the next tip");
            for tip in TIPS.iter().skip(2) {
                ui.advance(7_000).await;
                assert!(ui.has(tip), "expected the next tip");
            }
            // Past the last one it starts again.
            ui.advance(7_000).await;
            assert!(ui.has(TIPS[0]));
        });
    }
}
