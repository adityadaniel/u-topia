//! The guided tour shown in Settings → Help: one topic per feature, each with the
//! shortcuts that drive it and, where a picture helps, a screenshot of the real app.
//!
//! The screenshots are captured from the app itself against a mock graph
//! (`cargo run -p xtask -- help-shots`, see `assets/help/README.md`) and embedded here, so Help works
//! offline and inside a signed bundle with no resource lookup.

use gpui::{Image, ImageFormat};
use std::sync::Arc;

/// One section of the tour.
pub struct HelpTopic {
    pub title: &'static str,
    /// One line, shown in the topic list.
    pub summary: &'static str,
    /// Body paragraphs.
    pub body: &'static [&'static str],
    /// Catalog action names whose shortcuts drive this feature.
    pub actions: &'static [&'static str],
    /// Embedded PNG, captured from the app with mock data.
    pub shot: Option<&'static [u8]>,
}

macro_rules! shot {
    ($name:literal) => {
        Some(include_bytes!(concat!("../assets/help/", $name, ".png")).as_slice())
    };
}

pub const TOPICS: &[HelpTopic] = &[
    HelpTopic {
        title: "Daily Notes",
        summary: "Where the day starts: one page per calendar day, always ready to type in.",
        body: &[
            "Utopia opens on the Daily Notes feed: today at the top, then every earlier day \
             going back, each editable where it sits. Scroll up through the days and older \
             ones load as you reach them.",
            "A day only becomes a real page once you write in it, so visiting a date leaves \
             nothing behind. The left sidebar lists the days that have something in them, \
             either an entry of their own or a link from somewhere else.",
            "Press Daily Note to come back to the feed from anywhere. Today and Yesterday \
             open those two days on their own, away from the feed, which is handy when you \
             want the whole window for one day.",
        ],
        actions: &[
            "GoDailyNotes",
            "GoToday",
            "GoYesterday",
            "PrevDay",
            "NextDay",
        ],
        shot: shot!("daily-notes"),
    },
    HelpTopic {
        title: "Outlining",
        summary: "Every note is a tree of blocks. The block is the unit of everything.",
        body: &[
            "Enter starts a new block, Tab makes it a child of the one above and Shift+Tab \
             moves it back out. Blocks carry their children with them, so moving a block up \
             or down moves its whole subtree.",
            "Collapse a block to fold its children away. Zoom in on a block to make it the \
             top of the page and work on that branch alone; zoom out to step back up.",
            "Deleting a block keeps its identity in the trash, so anything that referenced \
             it still resolves instead of breaking.",
        ],
        actions: &[
            "Newline",
            "Indent",
            "Outdent",
            "MoveBlockUp",
            "MoveBlockDown",
            "ToggleCollapse",
            "ZoomIn",
            "ZoomOut",
            "DeleteBlock",
        ],
        shot: shot!("outline"),
    },
    HelpTopic {
        title: "Links, Tags And Backlinks",
        summary: "Type [[a name]] to link a page. The link works both ways.",
        body: &[
            "`[[Ana Lovelace]]` links a page and creates it on first use. `#project` and \
             `#[[big project]]` do exactly the same thing: a tag is a page link, so there is \
             no separate tag system to keep tidy.",
            "Every page shows its Linked References underneath: each block elsewhere that \
             points at it, grouped by page and editable in place. Unlinked References list \
             blocks that mention the title without linking it, with one click to link them.",
            "Dates are pages too, so `[[tomorrow]]`, `[[next sunday]]` and `[[2 weeks ago]]` \
             resolve to the right day and are rewritten to that day's title when the block is \
             committed.",
        ],
        actions: &["ToggleReferences"],
        shot: shot!("links"),
    },
    HelpTopic {
        title: "Block References And Embeds",
        summary: "Quote one block in many places without copying its text.",
        body: &[
            "Copy a block's reference to get its `((uid))`. Paste that anywhere and the block \
             renders in place, following the original as it is edited.",
            "`{{embed: ((uid))}}` pulls in the block and its children instead of just the one \
             line. Either way there is a single source of truth: edit it anywhere and every \
             place that shows it updates.",
            "The reference inspector in the right sidebar shows what a block points at, what \
             points back, and anything that has not resolved yet.",
        ],
        actions: &["CopyBlockRef", "ToggleRightSidebar"],
        shot: None,
    },
    HelpTopic {
        title: "Finding Things",
        summary: "Jump by name, or search every block.",
        body: &[
            "Quick Jump matches page titles, aliases and dates as you type. If what you typed \
             is not a page yet, it offers to create it, so capturing a new idea is the same \
             gesture as opening an old one.",
            "Search looks inside every block in the graph and shows which page each hit came \
             from. Hold Shift or Cmd when you pick a result to open it in the right sidebar \
             instead, keeping your place.",
        ],
        actions: &["QuickJump", "FullSearch"],
        shot: shot!("quick-jump"),
    },
    HelpTopic {
        title: "Block Types And The Slash Menu",
        summary: "To-dos, headings, quotes and code, from one menu.",
        body: &[
            "Type `/` in a block to pick what it should be: a to-do, a heading, a quote, a \
             code block, a numbered item, a query, a template or an attachment.",
            "Cycle To-do State turns the current block into a to-do, then ticks it, then puts \
             it back to plain text, so you can keep both hands on the keyboard.",
        ],
        actions: &["ToggleTodo", "Bold", "Italic", "Code"],
        shot: None,
    },
    HelpTopic {
        title: "Attributes And Queries",
        summary: "Write key:: value, then ask questions about it.",
        body: &[
            "A block of the form `status:: reading` records an attribute. Attributes show in \
             the page header and are queryable.",
            "`{{query: {and: [[book]] {attr: status reading}}}}` lists every block that \
             matches, refreshed as the graph changes. Queries are evaluated against SQLite, \
             so they stay quick as the graph grows.",
        ],
        actions: &[],
        shot: None,
    },
    HelpTopic {
        title: "Graph View",
        summary: "See the shape of what you have written.",
        body: &[
            "The graph view draws pages as nodes and links as edges, settling into a layout \
             you can pan and zoom. Switch between the whole graph and just the neighbourhood \
             of the page you are on, and hide daily pages when they crowd the picture.",
            "Click a node to open that page.",
        ],
        actions: &["ShowGraph"],
        shot: shot!("graph"),
    },
    HelpTopic {
        title: "Attachments And PDFs",
        summary: "Drop a file in. Highlight a PDF into your notes.",
        body: &[
            "Drag a file onto a block, or use the slash menu, and it is stored by the hash of \
             its contents, so the same file twice costs one copy. Images preview in place and \
             other files appear as chips.",
            "Open an attached PDF in the viewer and select text to keep it: the highlight is \
             saved as a quote block under the block you opened the PDF from, with a link back \
             to the page and position. Option-drag highlights an area instead of text.",
        ],
        actions: &[],
        shot: None,
    },
    HelpTopic {
        title: "Quick Capture",
        summary: "Put a thought in today's note without stopping what you are doing.",
        body: &[
            "Quick Capture opens one line, appends it to today's note and returns you to where \
             you were. There is a system-wide hotkey for it, and `utopia capture \"…\"` from a \
             terminal does the same thing.",
        ],
        actions: &["QuickCapture"],
        shot: None,
    },
    HelpTopic {
        title: "Sync, Export And Encryption",
        summary: "Your notes are files on your disk. They can also follow you.",
        body: &[
            "The graph is a single file you can copy, back up or put in version control. \
             Export writes plain Markdown, JSON and OPML, so nothing is trapped here.",
            "Sync runs against a relay or a shared folder and merges edits from several \
             machines without a server deciding who wins; turn on end-to-end encryption and \
             the relay only ever sees ciphertext.",
            "The graph file itself can be encrypted at rest, unlocked by a passphrase or from \
             the keychain.",
        ],
        actions: &[],
        shot: None,
    },
    HelpTopic {
        title: "Changing These Shortcuts",
        summary: "Every shortcut in the app can be rebound.",
        body: &[
            "The Keybindings tab lists every action. Click its key, press the combination you \
             want, and it applies at once and is remembered in `keymap.json` in the config \
             directory.",
            "Only the shortcuts you actually change are written down, so defaults that improve \
             in a later release still reach you. The ↺ beside a changed key puts it back; \
             Reset All puts everything back.",
            "If two actions end up on the same keys in the same place, the editor says so \
             rather than letting one quietly shadow the other.",
        ],
        actions: &["OpenSettings", "ShowShortcuts", "UtopiaHelp"],
        shot: None,
    },
];

/// Decode a topic's screenshot for gpui. `None` when the topic has no picture, or when the
/// embedded bytes are the 1x1 placeholder used before captures are generated.
pub fn shot_image(topic: &HelpTopic) -> Option<Arc<Image>> {
    let bytes = topic.shot?;
    // A real capture is thousands of bytes; the placeholder is ~70.
    if bytes.len() < 256 {
        return None;
    }
    Some(Arc::new(Image::from_bytes(
        ImageFormat::Png,
        bytes.to_vec(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::doc;

    #[test]
    fn every_referenced_action_is_in_the_catalog() {
        for t in TOPICS {
            for a in t.actions {
                assert!(
                    doc(a).is_some(),
                    "{} references unknown action {a}",
                    t.title
                );
            }
        }
    }

    #[test]
    fn topics_have_text() {
        for t in TOPICS {
            assert!(!t.title.is_empty());
            assert!(!t.summary.is_empty(), "{}", t.title);
            assert!(!t.body.is_empty(), "{}", t.title);
        }
    }

    /// Guards the help surface: every feature named in the app's scope gets a topic.
    #[test]
    fn the_tour_covers_the_feature_set() {
        let titles: Vec<&str> = TOPICS.iter().map(|t| t.title).collect();
        for expected in [
            "Daily Notes",
            "Outlining",
            "Links, Tags And Backlinks",
            "Block References And Embeds",
            "Finding Things",
            "Block Types And The Slash Menu",
            "Attributes And Queries",
            "Graph View",
            "Attachments And PDFs",
            "Quick Capture",
            "Sync, Export And Encryption",
            "Changing These Shortcuts",
        ] {
            assert!(titles.contains(&expected), "no topic for {expected}");
        }
    }
}
