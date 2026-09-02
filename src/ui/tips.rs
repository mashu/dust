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
