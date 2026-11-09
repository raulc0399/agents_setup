//! Text of an `ask`: which item it is about, the quoted content and the user's question. The
//! CLI prompt box receives this text as it is.
use crate::model::{single_line, terminal_safe};

/// Starts the reference line, followed by the item's number in the list and its title.
pub const ASK_REFERENCE_PREFIX: &str = "Item";

/// `Item <number>: <title>`, then every quoted line prefixed with `> `, then, when the user
/// typed one, the question after a blank line. As in the terminal list, the title is one line
/// and terminal controls in the content are escaped, so pasted text cannot drive the CLI.
pub fn ask_text(number: usize, title: &str, quote: &str, question: &str) -> String {
    let mut text = format!("{ASK_REFERENCE_PREFIX} {number}: {}", single_line(title));
    for line in terminal_safe(quote).lines() {
        text.push_str(&format!("\n> {line}"));
    }

    let question = terminal_safe(question.trim());
    if !question.is_empty() {
        text.push_str(&format!("\n\n{question}"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_every_line_and_appends_the_question() {
        let text = ask_text(
            3,
            "Collect",
            "Reads the device\nevery second",
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
            2,
            "Two\nlines\tand\x1b[31m",
            "red \x1b[31mtext\nnext\u{7}",
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
            ask_text(1, "Collect", "Reads the device", "  "),
            "Item 1: Collect\n> Reads the device"
        );
    }
}
