//! Text of an `ask`: which items it is about and the quoted content. The CLI prompt box
//! receives this text as it is, and the user types the question there. An explain request
//! carries the same reference with `EXPLAIN_LINE` below it.
use crate::model::{Intent, single_line, terminal_safe};
use serde::{Deserialize, Serialize};

/// Starts the reference line, followed by the item's number in the list and its title.
pub const ASK_REFERENCE_PREFIX: &str = "Item";

/// The line below the reference of an explain request; with the question's `intent`, it tells
/// the agent to explain the items and record the explanation.
pub const EXPLAIN_LINE: &str = "Explain";

/// One item an `ask` is about: its number in the list as shown, its title, and its text or
/// the part of it the user selected.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskItem {
    pub number: usize,
    pub title: String,
    pub quote: String,
}

/// Per item `Item <number>: <title>`, then every quoted line prefixed with `> `, the items one
/// after the other. The title is one line and terminal controls in the content are escaped,
/// so pasted text cannot drive the CLI.
pub fn ask_text(items: &[AskItem]) -> String {
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
    lines.join("\n")
}

/// The text a shared question carries: the reference of `ask_text`, and below it the line of
/// its intent, if any.
pub fn request_text(items: &[AskItem], intent: Option<Intent>) -> String {
    let reference = ask_text(items);
    match intent {
        Some(Intent::Explain) => format!("{reference}\n{EXPLAIN_LINE}"),
        None => reference,
    }
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
    fn quotes_every_line() {
        let text = ask_text(&[item(3, "Collect", "Reads the device\nevery second")]);
        assert_eq!(text, "Item 3: Collect\n> Reads the device\n> every second");
    }

    #[test]
    fn escapes_terminal_controls_and_keeps_the_title_on_one_line() {
        let text = ask_text(&[item(
            2,
            "Two\nlines\tand\x1b[31m",
            "red \x1b[31mtext\nnext\u{7}",
        )]);
        assert_eq!(
            text,
            "Item 2: Two lines and\\u{1b}[31m\n> red \\u{1b}[31mtext\n> next\\u{7}"
        );
    }

    #[test]
    fn an_explain_request_ends_with_the_explain_line() {
        let items = [item(3, "Collect", "Reads the device")];
        assert_eq!(
            request_text(&items, Some(Intent::Explain)),
            "Item 3: Collect\n> Reads the device\nExplain"
        );
        assert_eq!(request_text(&items, None), ask_text(&items));
    }

    #[test]
    fn several_items_follow_each_other() {
        let text = ask_text(&[
            item(2, "Collect", "Reads the device"),
            item(5, "Store", "Writes a batch\n\nonce a minute"),
        ]);
        assert_eq!(
            text,
            "Item 2: Collect\n> Reads the device\nItem 5: Store\n> Writes a batch\n> \n> once a minute"
        );
    }
}
