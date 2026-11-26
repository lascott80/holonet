//! Snippets (Snippets.dc.html): saved commands with `${name}` variables
//! that ask for a value on every run (`decisions.md`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::SnippetId;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snippet {
    pub id: SnippetId,
    pub name: String,
    pub command: String,
    /// "Press Enter after inserting".
    #[serde(default = "yes")]
    pub press_enter: bool,
    /// "Ask before running".
    #[serde(default)]
    pub confirm: bool,
}

fn yes() -> bool {
    true
}

/// A piece of a command: text as written, or a variable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Text(String),
    Variable(String),
}

/// Split a command into text and `${name}` variables. A name is letters,
/// digits and `_`. `$${x}` writes `${x}` literally; anything else that is
/// not a whole variable is text.
pub fn parse(command: &str) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut rest = command;
    while let Some(at) = rest.find('$') {
        text.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        if let Some(escaped) = after.strip_prefix("${") {
            // `$${` is a literal `${`.
            text.push_str("${");
            rest = escaped;
            continue;
        }
        if let Some(inner) = after.strip_prefix('{')
            && let Some(end) = inner.find('}')
        {
            let name = &inner[..end];
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                if !text.is_empty() {
                    parts.push(Part::Text(std::mem::take(&mut text)));
                }
                parts.push(Part::Variable(name.to_string()));
                rest = &inner[end + 1..];
                continue;
            }
        }
        text.push('$');
        rest = after;
    }
    text.push_str(rest);
    if !text.is_empty() {
        parts.push(Part::Text(text));
    }
    parts
}

impl Snippet {
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self { id: SnippetId::new(), name: name.into(), command: command.into(), press_enter: true, confirm: false }
    }

    /// The name to show; "Untitled snippet" when empty.
    pub fn display_name(&self) -> &str {
        let name = self.name.trim();
        if name.is_empty() { "Untitled snippet" } else { name }
    }

    /// The variables to ask for, each once, in the order they first appear.
    pub fn variables(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for part in parse(&self.command) {
            if let Part::Variable(name) = part
                && !names.contains(&name)
            {
                names.push(name);
            }
        }
        names
    }

    /// The command with each variable replaced by its value (missing ones
    /// by nothing).
    pub fn fill(&self, values: &HashMap<String, String>) -> String {
        parse(&self.command)
            .into_iter()
            .map(|part| match part {
                Part::Text(t) => t,
                Part::Variable(name) => values.get(&name).cloned().unwrap_or_default(),
            })
            .collect()
    }

    /// What to type into a terminal: the filled command with its line
    /// breaks as Enter (`\r`), plus a final Enter when "Press Enter after
    /// inserting" is on.
    pub fn input(&self, values: &HashMap<String, String>) -> String {
        let filled = self.fill(values).replace("\r\n", "\n");
        let mut text = filled.trim_end_matches('\n').replace('\n', "\r");
        if self.press_enter {
            text.push('\r');
        }
        text
    }

    /// The run note on library cards (Snippets.dc.html).
    pub fn run_note(&self) -> &'static str {
        if self.confirm {
            "Asks before running"
        } else if self.press_enter {
            "Runs immediately"
        } else {
            "Inserts without running"
        }
    }
}
