//! Bounded event history shared by the desktop and terminal interfaces.

use std::collections::VecDeque;

use crate::ops::Level;

const MAX_LINES: usize = 400;

#[derive(Default)]
pub(crate) struct Journal {
    lines: VecDeque<(Level, String)>,
}

impl Journal {
    pub(crate) fn push(&mut self, level: Level, line: String) {
        if self.lines.len() == MAX_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back((level, line));
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = &(Level, String)> {
        self.lines.iter()
    }

    pub(crate) fn text(&self) -> String {
        self.iter()
            .map(|(level, line)| format_line(*level, line))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub(crate) fn format_line(level: Level, line: &str) -> String {
    if level == Level::Step {
        format!("— {line}")
    } else {
        line.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_copy_includes_every_line_in_display_order() {
        let mut log = Journal::default();
        log.push(Level::Step, "First".into());
        log.push(Level::Info, "Middle".into());
        log.push(Level::Err, "Last".into());
        assert_eq!(log.text(), "— First\nMiddle\nLast");
    }

    #[test]
    fn journal_retains_the_latest_400_lines_in_order() {
        let mut log = Journal::default();
        assert!(log.is_empty());
        for number in 0..405 {
            log.push(Level::Info, number.to_string());
        }

        let retained = log.iter().map(|(_, line)| line.as_str());
        let expected: Vec<_> = (5..405).map(|number| number.to_string()).collect();
        assert!(retained.eq(expected.iter().map(String::as_str)));
    }
}
