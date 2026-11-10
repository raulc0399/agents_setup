//! Text of an `ask`: which items it is about, the quoted content and the user's question. The
//! CLI prompt box receives this text as it is.
use crate::model::{single_line, terminal_safe};
use serde::{Deserialize, Serialize};

/// Starts the reference line, followed by the item's number in the list and its title.
pub const ASK_REFERENCE_PREFIX: &str = "Item";

/// One item an `ask` is about: its number in the list as shown, its title, and its text or
/// the part of it the user selected.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskItem {
    pub number: usize,
    pub title: String,
    pub quote: String,
}

/// Per item `Item <number>: <title>`, then every quoted line prefixed with `> `; then, when the
/// user typed one, the question after a blank line. Items follow each other without a blank
/// line, so the first blank line always starts the question. One item gives the terminal
/// list's text. As there, the title is one line and terminal controls in the content are
/// escaped, so pasted text cannot drive the CLI.
pub fn ask_text(items: &[AskItem], question: &str) -> String {
    let mut lines = Vec::new();
    for item in items {
        lines.push(format!(
            "{ASK_REFERENCE_PREFIX} {}: {}",
            item.number,
            single_line(&item.title)
        ));
        for line in terminal_safe(&item.quote).lines() {
            lines.push(format!("> {line}"));
        }
    }
    let mut text = lines.join("\n");

    let question = terminal_safe(question.trim());
    if !question.is_empty() {
        text.push_str(&format!("\n\n{question}"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(number: usize, title: &str, quote: &str) -> AskItem {
        AskItem {
            number,
            title: title.into(),
            quote: quote.into(),
        }
    }

    #[test]
    fn quotes_every_line_and_appends_the_question() {
        let text = ask_text(
            &[item(3, "Collect", "Reads the device\nevery second")],
            " Why poll? \n",
        );
        assert_eq!(
            text,
            "Item 3: Collect\n> Reads the device\n> every second\n\nWhy poll?"
        );
    }

    #[test]
    fn escapes_terminal_controls_and_keeps_the_title_on_one_line() {
        let text = ask_text(
            &[item(
                2,
                "Two\nlines\tand\x1b[31m",
                "red \x1b[31mtext\nnext\u{7}",
            )],
            "ok\x08?",
        );
        assert_eq!(
            text,
            "Item 2: Two lines and\\u{1b}[31m\n> red \\u{1b}[31mtext\n> next\\u{7}\n\nok\\u{8}?"
        );
    }

    #[test]
    fn without_a_question_shares_the_reference_only() {
        assert_eq!(
            ask_text(&[item(1, "Collect", "Reads the device")], "  "),
            "Item 1: Collect\n> Reads the device"
        );
    }

    #[test]
    fn several_items_follow_each_other_before_the_question() {
        let text = ask_text(
            &[
                item(2, "Collect", "Reads the device"),
                item(5, "Store", "Writes a batch\n\nonce a minute"),
            ],
            "How do these relate?",
        );
        assert_eq!(
            text,
            "Item 2: Collect\n> Reads the device\nItem 5: Store\n> Writes a batch\n> \n> once a minute\n\nHow do these relate?"
        );
    }
}
