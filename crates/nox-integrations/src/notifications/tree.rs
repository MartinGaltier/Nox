// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/Notch/NotchNotificationReaderCore.swift and NotchNotificationSupport.swift, commit e916961.

//! Banner discovery and content extraction over an accessibility tree,
//! generic so it runs on synthetic trees in tests. Budgets bound the walk
//! (depth, node count) so a busy Notification Center cannot stall a scan.

pub const BANNER_ROLES: &[&str] = &["AXNotificationCenterAlert", "AXNotificationCenterBanner"];
pub const STACK_ROLES: &[&str] = &[
    "AXNotificationCenterAlertStack",
    "AXNotificationCenterBannerStack",
];
const MAX_DEPTH: usize = 10;
const MAX_NODES: usize = 384;
const MAX_CHILDREN: usize = 64;
const MAX_TEXT: usize = 16_384;

/// What the traversal needs from an accessibility element.
pub trait Node: Sized {
    fn attr(&self, name: &str) -> Option<String>;
    fn children(&self) -> Vec<Self>;
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Content {
    pub app: String,
    pub title: String,
    pub subtitle: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub identifier: String,
    pub value: String,
}

/// Turns the static texts of one card into a notification. A card holding
/// several messages (two titles…) is rejected rather than flattened.
pub fn content_from(texts: &[Text]) -> Option<Content> {
    let vals = |id: &str| -> Vec<String> {
        texts
            .iter()
            .filter(|t| t.identifier == id)
            .map(|t| t.value.trim().to_owned())
            .filter(|v| !v.is_empty())
            .collect()
    };
    if texts.iter().any(|t| t.value.len() > MAX_TEXT) {
        return None;
    }
    if texts.iter().all(|t| t.identifier.is_empty())
        && (2..=3).contains(&texts.len())
        && texts.iter().all(|t| !t.value.trim().is_empty())
    {
        return Some(Content {
            app: String::new(),
            title: texts[0].value.trim().to_owned(),
            subtitle: if texts.len() == 3 {
                texts[1].value.trim().to_owned()
            } else {
                String::new()
            },
            body: texts[texts.len() - 1].value.trim().to_owned(),
        });
    }
    let (h, t, s, b) = (
        vals("header"),
        vals("title"),
        vals("subtitle"),
        vals("body"),
    );
    if h.len() > 1 || t.len() != 1 || s.len() > 1 || b.len() > 1 {
        return None;
    }
    Some(Content {
        app: h.into_iter().next().unwrap_or_default(),
        title: t[0].clone(),
        subtitle: s.into_iter().next().unwrap_or_default(),
        body: b.into_iter().next().unwrap_or_default(),
    })
}

pub struct Walker {
    nodes: usize,
    /// Budget exhausted or tree too deep: the scan is not authoritative.
    pub incomplete: bool,
}

impl Default for Walker {
    fn default() -> Self {
        Self {
            nodes: MAX_NODES,
            incomplete: false,
        }
    }
}

/// A banner found in the tree; `transient` = banner (vs persistent alert).
pub struct Found<N> {
    pub root: N,
    pub transient: bool,
    pub content: Option<Content>,
    /// Image labels (app icon descriptions) for resolving the source app.
    pub image_labels: Vec<String>,
}

impl Walker {
    fn visit(&mut self, depth: usize) -> bool {
        if depth > MAX_DEPTH || self.nodes == 0 {
            self.incomplete = true;
            return false;
        }
        self.nodes -= 1;
        true
    }

    fn kids<N: Node>(&mut self, n: &N) -> Vec<N> {
        let c = n.children();
        if c.len() > MAX_CHILDREN {
            self.incomplete = true;
            return vec![];
        }
        c
    }

    pub fn content<N: Node>(&mut self, root: &N) -> (Option<Content>, Vec<String>) {
        let mut texts = vec![];
        let mut images = vec![];
        self.walk(root, 0, &mut texts, &mut images);
        (content_from(&texts), images)
    }

    fn walk<N: Node>(
        &mut self,
        n: &N,
        depth: usize,
        texts: &mut Vec<Text>,
        images: &mut Vec<String>,
    ) {
        if !self.visit(depth) {
            return;
        }
        match n.attr("AXRole").as_deref() {
            Some("AXStaticText") => {
                let id = n.attr("AXIdentifier").unwrap_or_default();
                if ["", "header", "title", "subtitle", "body"].contains(&id.as_str())
                    && let Some(v) = n.attr("AXValue")
                {
                    texts.push(Text {
                        identifier: id,
                        value: v,
                    });
                }
            }
            Some("AXImage") => {
                if let Some(d) = n
                    .attr("AXDescription")
                    .filter(|d| !d.is_empty() && d.len() <= 256)
                {
                    images.push(d);
                }
            }
            _ => {}
        }
        for c in self.kids(n) {
            self.walk(&c, depth + 1, texts, images);
        }
    }

    /// Banners and alert cards under `n` (a Notification Center window).
    pub fn find<N: Node + Clone>(&mut self, n: &N, depth: usize, out: &mut Vec<Found<N>>) {
        if !self.visit(depth) {
            return;
        }
        let subrole = n.attr("AXSubrole").unwrap_or_default();
        if BANNER_ROLES.contains(&subrole.as_str()) {
            let (content, image_labels) = self.content(n);
            out.push(Found {
                root: n.clone(),
                transient: subrole == "AXNotificationCenterBanner",
                content,
                image_labels,
            });
            return;
        }
        if STACK_ROLES.contains(&subrole.as_str()) {
            let transient = subrole == "AXNotificationCenterBannerStack";
            for c in self.kids(n) {
                self.stack_card(&c, depth + 1, transient, out);
            }
            return;
        }
        for c in self.kids(n) {
            self.find(&c, depth + 1, out);
        }
    }

    fn stack_card<N: Node + Clone>(
        &mut self,
        n: &N,
        depth: usize,
        transient: bool,
        out: &mut Vec<Found<N>>,
    ) {
        if !self.visit(depth) {
            return;
        }
        let (content, image_labels) = self.content(n);
        if let Some(card) = content {
            out.push(Found {
                root: n.clone(),
                transient,
                content: Some(card),
                image_labels,
            });
            return;
        }
        for c in self.kids(n) {
            self.stack_card(&c, depth + 1, transient, out);
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Synthetic accessibility node.
    #[derive(Clone, Default)]
    pub struct Fake {
        pub attrs: Vec<(&'static str, String)>,
        pub kids: Vec<Fake>,
    }

    impl Node for Fake {
        fn attr(&self, name: &str) -> Option<String> {
            self.attrs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.clone())
        }
        fn children(&self) -> Vec<Self> {
            self.kids.clone()
        }
    }

    pub fn text(id: &str, v: &str) -> Fake {
        Fake {
            attrs: vec![
                ("AXRole", "AXStaticText".into()),
                ("AXIdentifier", id.into()),
                ("AXValue", v.into()),
            ],
            kids: vec![],
        }
    }

    pub fn banner(subrole: &str, kids: Vec<Fake>) -> Fake {
        Fake {
            attrs: vec![("AXRole", "AXGroup".into()), ("AXSubrole", subrole.into())],
            kids,
        }
    }

    pub fn window(kids: Vec<Fake>) -> Fake {
        Fake {
            attrs: vec![("AXRole", "AXWindow".into())],
            kids: vec![Fake {
                attrs: vec![("AXRole", "AXScrollArea".into())],
                kids,
            }],
        }
    }

    #[test]
    fn parses_identified_and_anonymous_texts() {
        let c = content_from(&[
            Text {
                identifier: "header".into(),
                value: "Messages".into(),
            },
            Text {
                identifier: "title".into(),
                value: "Synthetic Sender".into(),
            },
            Text {
                identifier: "body".into(),
                value: "Synthetic body".into(),
            },
        ])
        .unwrap();
        assert_eq!(
            (c.app.as_str(), c.title.as_str(), c.body.as_str()),
            ("Messages", "Synthetic Sender", "Synthetic body")
        );
        let c = content_from(&[
            Text {
                identifier: "".into(),
                value: "T".into(),
            },
            Text {
                identifier: "".into(),
                value: "S".into(),
            },
            Text {
                identifier: "".into(),
                value: "B".into(),
            },
        ])
        .unwrap();
        assert_eq!(c.subtitle, "S");
        assert!(
            content_from(&[
                Text {
                    identifier: "title".into(),
                    value: "A".into()
                },
                Text {
                    identifier: "title".into(),
                    value: "B".into()
                },
            ])
            .is_none(),
            "two messages are never flattened"
        );
    }

    #[test]
    fn finds_banners_and_stack_cards() {
        let tree = window(vec![
            banner(
                "AXNotificationCenterBanner",
                vec![
                    text("header", "Calendrier"),
                    text("title", "Synthetic event"),
                    text("body", "Dans 5 min"),
                ],
            ),
            banner(
                "AXNotificationCenterAlertStack",
                vec![
                    Fake {
                        kids: vec![text("title", "Card 1"), text("body", "b1")],
                        ..Default::default()
                    },
                    Fake {
                        kids: vec![text("title", "Card 2")],
                        ..Default::default()
                    },
                ],
            ),
        ]);
        let mut w = Walker::default();
        let mut out = vec![];
        w.find(&tree, 0, &mut out);
        assert!(!w.incomplete);
        let titles: Vec<_> = out
            .iter()
            .map(|f| f.content.as_ref().unwrap().title.clone())
            .collect();
        assert_eq!(titles, vec!["Synthetic event", "Card 1", "Card 2"]);
        assert!(out[0].transient);
        assert!(!out[1].transient, "alert stack cards are persistent");
    }

    #[test]
    fn budget_marks_incomplete() {
        let mut deep = text("title", "x");
        for _ in 0..20 {
            deep = Fake {
                kids: vec![deep],
                ..Default::default()
            };
        }
        let mut w = Walker::default();
        let mut out = vec![];
        w.find(&deep, 0, &mut out);
        assert!(w.incomplete);
    }
}
