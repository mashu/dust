//! Group alignment via edit distance for letter-level scoring.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlignmentPair {
    pub sent_char: Option<char>,
    pub received_char: Option<char>,
    pub matched: bool,
}

pub fn align_group(sent: &str, received: &str) -> Vec<AlignmentPair> {
    let s: Vec<char> = sent.to_ascii_uppercase().chars().collect();
    let r: Vec<char> = received.to_ascii_uppercase().chars().collect();

    if s.is_empty() && r.is_empty() {
        return Vec::new();
    }
    if s.is_empty() {
        return r
            .into_iter()
            .map(|ch| AlignmentPair {
                sent_char: None,
                received_char: Some(ch),
                matched: false,
            })
            .collect();
    }
    if r.is_empty() {
        return s
            .into_iter()
            .map(|ch| AlignmentPair {
                sent_char: Some(ch),
                received_char: None,
                matched: false,
            })
            .collect();
    }

    let m = s.len();
    let n = r.len();
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in 0..=m {
        dp[i][0] = i;
    }
    for j in 0..=n {
        dp[0][j] = j;
    }
    for i in 1..=m {
        for j in 1..=n {
            let match_cost = if s[i - 1] == r[j - 1] { 0 } else { 1 };
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + match_cost);
        }
    }

    let mut alignment = Vec::new();
    let mut i = m;
    let mut j = n;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 {
            let match_cost = if s[i - 1] == r[j - 1] { 0 } else { 1 };
            let current = dp[i][j];
            let substitution = dp[i - 1][j - 1] + match_cost;
            let deletion = dp[i - 1][j] + 1;
            if current == substitution {
                alignment.push(AlignmentPair {
                    sent_char: Some(s[i - 1]),
                    received_char: Some(r[j - 1]),
                    matched: s[i - 1] == r[j - 1],
                });
                i -= 1;
                j -= 1;
            } else if current == deletion {
                alignment.push(AlignmentPair {
                    sent_char: Some(s[i - 1]),
                    received_char: None,
                    matched: false,
                });
                i -= 1;
            } else {
                alignment.push(AlignmentPair {
                    sent_char: None,
                    received_char: Some(r[j - 1]),
                    matched: false,
                });
                j -= 1;
            }
        } else if i > 0 {
            alignment.push(AlignmentPair {
                sent_char: Some(s[i - 1]),
                received_char: None,
                matched: false,
            });
            i -= 1;
        } else {
            alignment.push(AlignmentPair {
                sent_char: None,
                received_char: Some(r[j - 1]),
                matched: false,
            });
            j -= 1;
        }
    }
    alignment.reverse();
    alignment
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LetterAccuracy {
    pub correct: u32,
    pub total: u32,
}

pub fn calculate_group_letter_accuracy(
    groups: &[(String, String)],
) -> std::collections::BTreeMap<char, LetterAccuracy> {
    let mut letter_accuracy = std::collections::BTreeMap::new();
    for (sent, received) in groups {
        for pair in align_group(sent, received) {
            if let Some(ch) = pair.sent_char {
                let entry = letter_accuracy.entry(ch).or_insert(LetterAccuracy {
                    correct: 0,
                    total: 0,
                });
                entry.total += 1;
                if pair.matched {
                    entry.correct += 1;
                }
            }
        }
    }
    letter_accuracy
}

pub fn calculate_overall_character_accuracy(groups: &[(String, String)]) -> f64 {
    let mut total_sent = 0u32;
    let mut correct = 0u32;
    for (sent, received) in groups {
        for pair in align_group(sent, received) {
            if pair.sent_char.is_some() {
                total_sent += 1;
                if pair.matched {
                    correct += 1;
                }
            }
        }
    }
    if total_sent == 0 {
        0.0
    } else {
        f64::from(correct) / f64::from(total_sent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn align_substitution() {
        let a = align_group("ABC", "ABD");
        assert_eq!(a.len(), 3);
        assert!(a[0].matched && a[1].matched);
        assert!(!a[2].matched);
        assert_eq!(a[2].sent_char, Some('C'));
        assert_eq!(a[2].received_char, Some('D'));
    }

    #[test]
    fn empty_received_is_all_deletions() {
        let a = align_group("KM", "");
        assert_eq!(a.len(), 2);
        assert!(a.iter().all(|p| p.received_char.is_none() && !p.matched));
    }

    #[test]
    fn overall_accuracy_counts_sent_letters() {
        let groups = vec![
            ("ABC".to_string(), "ABC".to_string()),
            ("DE".to_string(), "DX".to_string()),
        ];
        let acc = calculate_overall_character_accuracy(&groups);
        assert!((acc - 0.8).abs() < 1e-9);
    }
}
