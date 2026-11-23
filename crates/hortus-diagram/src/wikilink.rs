//! 节点标签里的 Obsidian 双链：剥离 `[[` `]]` 得到展示文字，并保留目标与标题。
//!
//! 只处理节点标签本身；嵌入 `![[`、转义 `\[[`、未闭合写法与
//! `state X [[choice]]` 这类 Mermaid 伪状态后缀都不算链接。

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WikiLink {
    /// 去掉首条链接标记后的完整标签文字，其余文本原样保留。
    pub display: String,
    pub target: String,
    pub heading: Option<String>,
}

/// 提取标签中的首条合法双链；没有时返回 `None`，调用方沿用原文字排版。
pub(crate) fn extract(label: &str) -> Option<WikiLink> {
    let mut cursor = 0;
    while let Some(found) = label[cursor..].find("[[") {
        let start = cursor + found;
        let close = label[start + 2..].find("]]")?;
        let end = start + 2 + close;
        let inner = &label[start + 2..end];
        cursor = end + 2;
        if inner.contains("[[") {
            // 前一个 `[[` 未闭合，从它后面重新找真正闭合的链接。
            cursor = start + 2;
            continue;
        }
        if !can_open(label, start) || inner.contains('\n') || inner.contains('\r') {
            continue;
        }
        if let Some(link) = parse(label, start, inner) {
            return Some(link);
        }
    }
    None
}

/// 标签排版与测宽使用的文字；无链接时借用原文，避免每次分配。
pub(crate) fn display(label: &str) -> std::borrow::Cow<'_, str> {
    extract(label).map_or(std::borrow::Cow::Borrowed(label), |link| {
        std::borrow::Cow::Owned(link.display)
    })
}

fn can_open(label: &str, start: usize) -> bool {
    let prefix = &label.as_bytes()[..start];
    let backslashes = prefix
        .iter()
        .rev()
        .take_while(|&&byte| byte == b'\\')
        .count();
    prefix.last() != Some(&b'!') && backslashes.is_multiple_of(2)
}

fn parse(label: &str, start: usize, inner: &str) -> Option<WikiLink> {
    let (link, alias) = match inner.split_once('|') {
        Some((link, alias)) => (link, Some(alias.trim()).filter(|value| !value.is_empty())),
        None => (inner, None),
    };
    let (target, heading) = match link.split_once('#') {
        Some((target, heading)) => (
            target.trim(),
            Some(heading.trim()).filter(|value| !value.is_empty()),
        ),
        None => (link.trim(), None),
    };
    if target.is_empty() || is_state_pseudo(label, start, target, heading, alias) {
        return None;
    }
    let shown = match alias {
        Some(alias) => alias.to_owned(),
        None => match heading {
            Some(heading) => format!("{target}#{heading}"),
            None => target.to_owned(),
        },
    };
    let mut display = String::with_capacity(label.len() + shown.len());
    display.push_str(&label[..start]);
    display.push_str(&shown);
    display.push_str(&label[start + 2 + inner.len() + 2..]);
    Some(WikiLink {
        display,
        target: target.to_owned(),
        heading: heading.map(str::to_owned),
    })
}

fn is_state_pseudo(
    label: &str,
    start: usize,
    target: &str,
    heading: Option<&str>,
    alias: Option<&str>,
) -> bool {
    if heading.is_some() || alias.is_some() || !matches!(target, "choice" | "fork" | "join") {
        return false;
    }
    let mut prefix = label[..start].trim_start();
    while let Some(rest) = prefix.strip_prefix('>') {
        prefix = rest.trim_start();
    }
    let Some(rest) = prefix.strip_prefix("state") else {
        return false;
    };
    if !rest.starts_with(char::is_whitespace) {
        return false;
    }
    let id = rest.trim();
    !id.is_empty() && id.chars().all(|ch| !ch.is_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_target_alias_and_heading_are_normalized() {
        let link = extract("[[Target]]").unwrap();
        assert_eq!(link.display, "Target");
        assert_eq!(link.target, "Target");
        assert_eq!(link.heading, None);

        let link = extract("[[Target|Alias]]").unwrap();
        assert_eq!(link.display, "Alias");
        assert_eq!(link.heading, None);

        let link = extract("[[Target#Heading]]").unwrap();
        assert_eq!(link.display, "Target#Heading");
        assert_eq!(link.heading.as_deref(), Some("Heading"));

        let link = extract("[[Target#Heading|Alias]]").unwrap();
        assert_eq!(link.display, "Alias");
        assert_eq!(link.target, "Target");
        assert_eq!(link.heading.as_deref(), Some("Heading"));
    }

    #[test]
    fn surrounding_text_and_later_links_survive() {
        let link = extract("before [[Target|Alias]] after [[Other]]").unwrap();
        assert_eq!(link.display, "before Alias after [[Other]]");
        assert_eq!(link.target, "Target");

        let link = extract("前缀😀[[目标#章节|别名]]后缀").unwrap();
        assert_eq!(link.display, "前缀😀别名后缀");
        assert_eq!(link.target, "目标");
        assert_eq!(link.heading.as_deref(), Some("章节"));
    }

    #[test]
    fn embeds_escapes_and_malformed_links_are_ignored() {
        assert!(extract("![[picture.png]]").is_none());
        assert!(extract("![[picture.png]] [[Target]]").is_some_and(|l| l.target == "Target"));
        assert!(extract("\\[[Target]]").is_none());
        assert!(extract("[[unclosed").is_none());
        assert!(extract("[[]]").is_none());
        assert!(extract("[[#Heading]]").is_none());
        assert!(extract("[[|Alias]]").is_none());
        assert!(extract("[[unclosed [[Target]]").is_some_and(|l| l.target == "Target"));
    }

    #[test]
    fn only_odd_backslash_runs_escape_wiki_links() {
        assert!(extract(r"\[[Target]]").is_none());
        assert_eq!(extract(r"\\[[Target]]").unwrap().target, "Target");
        assert!(extract(r"\\\[[Target]]").is_none());
    }

    #[test]
    fn mermaid_state_pseudo_suffix_is_not_a_link() {
        assert!(extract("state Picker [[choice]]").is_none());
        assert!(extract("state Picker [[fork]]").is_none());
        assert!(extract("state Picker [[join]]").is_none());
        // 其他目标和带别名的写法仍是真实双链。
        assert!(extract("state Picker [[choice|Pick]]").is_some_and(|l| l.target == "choice"));
        assert!(extract("[[choice]]").is_some_and(|l| l.target == "choice"));
    }

    fn scene(source: &str) -> crate::Scene {
        let diagram = crate::parse_diagram(source).unwrap();
        let options = crate::LayoutOptions::default();
        let mut fonts = crate::DeterministicMetrics;
        match diagram {
            crate::Diagram::Tree(tree) => crate::layout(&tree, &options, &mut fonts).unwrap(),
            crate::Diagram::Graph(graph) => {
                crate::layout_graph(&graph, &options, &mut fonts)
                    .unwrap()
                    .scene
            }
            crate::Diagram::Sequence(sequence) => {
                crate::layout_sequence(&sequence, &options, &mut fonts).unwrap()
            }
            crate::Diagram::State(state) => {
                crate::layout_state(&state, &options, &mut fonts).unwrap()
            }
            crate::Diagram::Class(class) => {
                crate::layout_class(&class, &options, &mut fonts).unwrap()
            }
            crate::Diagram::Er(er) => crate::layout_er(&er, &options, &mut fonts).unwrap(),
        }
    }

    fn assert_linked(source: &str, target: &str, heading: Option<&str>, label: &str) {
        let scene = scene(source);
        let hit = scene
            .hit_regions
            .iter()
            .find(|hit| {
                hit.link_target.as_deref() == Some(target) && hit.link_heading.as_deref() == heading
            })
            .unwrap_or_else(|| {
                panic!(
                    "no linked hit in {source}: {:?}",
                    scene
                        .hit_regions
                        .iter()
                        .map(|hit| (&hit.label, &hit.link_target, &hit.link_heading))
                        .collect::<Vec<_>>()
                )
            });
        assert_eq!(hit.label, label);
        let raw = &source[hit.source_range.clone()];
        assert!(raw.contains("[["));
        assert!(raw.contains(target));
        if let Some(heading) = heading {
            assert!(raw.contains(heading));
        }
    }

    #[test]
    fn every_diagram_type_exposes_links_on_its_nodes() {
        assert_linked(
            "mindmap\n  root(([[architecture]]))\n    [[design#Section Two|Design Spec]]\n      Child\n",
            "architecture",
            None,
            "architecture",
        );
        assert_linked(
            "mindmap\n  root(([[architecture]]))\n    [[design#Section Two|Design Spec]]\n      Child\n",
            "design",
            Some("Section Two"),
            "Design Spec",
        );
        assert_linked(
            "flowchart TB\n  A[\"[[design#Section Two|Design Spec]]\"] --> B[\"[[architecture]]\"]\n",
            "design",
            Some("Section Two"),
            "Design Spec",
        );
        assert_linked(
            "sequenceDiagram\n  participant A as [[architecture]]\n  participant B as [[design#Section Two|Design Spec]]\n  A->>B: [[architecture]]\n",
            "design",
            Some("Section Two"),
            "Design Spec",
        );
        assert_linked(
            "stateDiagram-v2\n  state \"[[design#Section Two|Design Spec]]\" as Spec\n  [*] --> Spec\n",
            "design",
            Some("Section Two"),
            "Design Spec",
        );
        assert_linked(
            "classDiagram\n  class Design[\"[[design#Section Two|Design Spec]]\"] {\n    +id: UUID\n  }\n",
            "design",
            Some("Section Two"),
            "Design Spec",
        );
        assert_linked(
            "erDiagram\n  DESIGN[\"[[design#Section Two|Design Spec]]\"] {\n    bigint id PK\n  }\n",
            "design",
            Some("Section Two"),
            "Design Spec",
        );
    }

    #[test]
    fn embeds_and_pseudo_states_keep_their_literal_text() {
        let scene = scene("flowchart TB\n  A[\"![[architecture]]\"] --> B[\"plain\"]\n");
        let embed = scene
            .hit_regions
            .iter()
            .find(|hit| hit.label.contains("![[architecture]]"))
            .unwrap();
        assert_eq!(embed.link_target, None);
        assert_eq!(embed.link_heading, None);
    }

    #[test]
    fn unlinked_scenes_serialize_without_link_fields() {
        let scene = scene("flowchart TB\n  A[Alpha] --> B[Beta]\n").semantic_scene();
        let json = serde_json::to_string(&scene).unwrap();
        assert!(!json.contains("link_target"));
        assert!(!json.contains("link_heading"));
    }
}
