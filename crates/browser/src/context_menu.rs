//! App-rendered context menus: in windowless (OSR) mode the engine cannot
//! show its own menu, so the request is intercepted in the engine layer
//! (`context_menu_handler.rs`) and the browser view renders a Zed-style menu.
//! This module is the platform-neutral half: the page context extracted from
//! the engine and its mapping to menu items, kept as a pure function so the
//! mapping is unit-testable without a window or an engine.

/// Page context at the right-click point, extracted from the engine's
/// context-menu parameters
/// (`Glass:crates/browser/src/context_menu_handler.rs:19`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ContextMenuContext {
    pub link_url: Option<String>,
    pub selection_text: Option<String>,
    pub is_editable: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub can_cut: bool,
    pub can_copy: bool,
    pub can_paste: bool,
    pub can_delete: bool,
    pub can_select_all: bool,
}

/// One browser-view operation a context-menu entry triggers.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum MenuCommand {
    OpenLinkInNewTab(String),
    CopyLinkAddress(String),
    DownloadLink(String),
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Delete,
    SelectAll,
    GoBack,
    GoForward,
    Reload,
    Inspect,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum MenuItem {
    Separator,
    Entry {
        label: &'static str,
        command: MenuCommand,
    },
}

fn entry(label: &'static str, command: MenuCommand) -> MenuItem {
    MenuItem::Entry { label, command }
}

/// Map the page context to the menu shown, mirroring Glass's menu
/// (`Glass:crates/browser/src/browser_view/context_menu.rs:30`): a link
/// block, then either the editable-field edit actions or a plain selection's
/// Copy, then page navigation when nothing more specific applies, and always
/// Inspect last (DevTools, ticket #20).
pub(crate) fn context_menu_model(context: &ContextMenuContext) -> Vec<MenuItem> {
    let mut items = Vec::new();
    let has_link = context.link_url.is_some();
    let has_selection = context.selection_text.is_some();

    if let Some(link_url) = &context.link_url {
        items.push(entry(
            "Open Link in New Tab",
            MenuCommand::OpenLinkInNewTab(link_url.clone()),
        ));
        items.push(entry(
            "Copy Link Address",
            MenuCommand::CopyLinkAddress(link_url.clone()),
        ));
        items.push(entry(
            "Download Link",
            MenuCommand::DownloadLink(link_url.clone()),
        ));
        items.push(MenuItem::Separator);
    }

    if context.is_editable {
        if context.can_undo {
            items.push(entry("Undo", MenuCommand::Undo));
        }
        if context.can_redo {
            items.push(entry("Redo", MenuCommand::Redo));
        }
        items.push(MenuItem::Separator);
        if context.can_cut {
            items.push(entry("Cut", MenuCommand::Cut));
        }
        if context.can_copy {
            items.push(entry("Copy", MenuCommand::Copy));
        }
        if context.can_paste {
            items.push(entry("Paste", MenuCommand::Paste));
        }
        if context.can_delete {
            items.push(entry("Delete", MenuCommand::Delete));
        }
        items.push(MenuItem::Separator);
        if context.can_select_all {
            items.push(entry("Select All", MenuCommand::SelectAll));
        }
    } else if has_selection {
        items.push(entry("Copy", MenuCommand::Copy));
    }

    if !has_link && !has_selection && !context.is_editable {
        items.push(entry("Back", MenuCommand::GoBack));
        items.push(entry("Forward", MenuCommand::GoForward));
        items.push(entry("Reload", MenuCommand::Reload));
    }

    let mut items = normalize_separators(items);
    if items.is_empty() {
        // An editable field that reports no edit capabilities would otherwise
        // yield only Inspect, which is too little to right-click for.
        items = vec![
            entry("Back", MenuCommand::GoBack),
            entry("Forward", MenuCommand::GoForward),
            entry("Reload", MenuCommand::Reload),
        ];
    }
    items.push(MenuItem::Separator);
    items.push(entry("Inspect", MenuCommand::Inspect));
    items
}

/// Drop leading, trailing, and doubled separators left by skipped optional
/// entries.
fn normalize_separators(items: Vec<MenuItem>) -> Vec<MenuItem> {
    let mut normalized: Vec<MenuItem> = Vec::with_capacity(items.len());
    for item in items {
        if item == MenuItem::Separator
            && normalized.last().is_none_or(|last| *last == MenuItem::Separator)
        {
            continue;
        }
        normalized.push(item);
    }
    if normalized.last() == Some(&MenuItem::Separator) {
        normalized.pop();
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(items: &[MenuItem]) -> Vec<&'static str> {
        items
            .iter()
            .map(|item| match item {
                MenuItem::Separator => "—",
                MenuItem::Entry { label, .. } => label,
            })
            .collect()
    }

    #[test]
    fn link_context_offers_link_actions() {
        let model = context_menu_model(&ContextMenuContext {
            link_url: Some("https://example.com/a".into()),
            ..Default::default()
        });

        assert_eq!(
            labels(&model),
            vec![
                "Open Link in New Tab",
                "Copy Link Address",
                "Download Link",
                "—",
                "Inspect",
            ],
        );
        assert_eq!(
            model[0],
            MenuItem::Entry {
                label: "Open Link in New Tab",
                command: MenuCommand::OpenLinkInNewTab("https://example.com/a".into()),
            },
        );
        assert_eq!(
            model[2],
            MenuItem::Entry {
                label: "Download Link",
                command: MenuCommand::DownloadLink("https://example.com/a".into()),
            },
        );
    }

    #[test]
    fn selection_context_offers_copy() {
        let model = context_menu_model(&ContextMenuContext {
            selection_text: Some("some words".into()),
            can_copy: true,
            ..Default::default()
        });

        assert_eq!(labels(&model), vec!["Copy", "—", "Inspect"]);
        assert_eq!(
            model[0],
            MenuItem::Entry {
                label: "Copy",
                command: MenuCommand::Copy,
            },
        );
    }

    #[test]
    fn editable_context_offers_supported_edit_actions_only() {
        let model = context_menu_model(&ContextMenuContext {
            is_editable: true,
            can_undo: true,
            can_cut: true,
            can_copy: true,
            can_paste: true,
            can_select_all: true,
            ..Default::default()
        });

        assert_eq!(
            labels(&model),
            vec![
                "Undo",
                "—",
                "Cut",
                "Copy",
                "Paste",
                "—",
                "Select All",
                "—",
                "Inspect",
            ],
        );
        assert!(
            !model.iter().any(|item| matches!(
                item,
                MenuItem::Entry {
                    command: MenuCommand::Redo | MenuCommand::Delete,
                    ..
                }
            )),
            "unsupported edit actions stay out of the menu"
        );
    }

    #[test]
    fn editable_selection_gets_edit_actions_not_the_plain_copy() {
        let model = context_menu_model(&ContextMenuContext {
            selection_text: Some("selected".into()),
            is_editable: true,
            can_copy: true,
            ..Default::default()
        });

        assert_eq!(labels(&model), vec!["Copy", "—", "Inspect"]);
        assert_eq!(
            model[0],
            MenuItem::Entry {
                label: "Copy",
                command: MenuCommand::Copy,
            },
        );
    }

    #[test]
    fn link_inside_editable_field_offers_both_blocks() {
        let model = context_menu_model(&ContextMenuContext {
            link_url: Some("https://example.com".into()),
            is_editable: true,
            can_paste: true,
            can_select_all: true,
            ..Default::default()
        });

        assert_eq!(
            labels(&model),
            vec![
                "Open Link in New Tab",
                "Copy Link Address",
                "Download Link",
                "—",
                "Paste",
                "—",
                "Select All",
                "—",
                "Inspect",
            ],
        );
    }

    #[test]
    fn plain_page_context_offers_navigation() {
        let model = context_menu_model(&ContextMenuContext::default());

        assert_eq!(
            labels(&model),
            vec!["Back", "Forward", "Reload", "—", "Inspect"]
        );
        assert_eq!(
            model[2],
            MenuItem::Entry {
                label: "Reload",
                command: MenuCommand::Reload,
            },
        );
        assert_eq!(
            model.last(),
            Some(&MenuItem::Entry {
                label: "Inspect",
                command: MenuCommand::Inspect,
            }),
            "every context ends with Inspect (ticket #20)",
        );
    }

    #[test]
    fn editable_context_with_no_capabilities_falls_back_to_navigation() {
        let model = context_menu_model(&ContextMenuContext {
            is_editable: true,
            ..Default::default()
        });

        assert_eq!(
            labels(&model),
            vec!["Back", "Forward", "Reload", "—", "Inspect"]
        );
    }
}
