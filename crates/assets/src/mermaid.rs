//! Bounded native Mermaid subset. Call only inside the isolated Mermaid worker.
//! Adapted from mdviewer (MIT, Copyright (c) 2026 Anthony GARCIA).

use crate::Preview;
mod svg;
pub const MAX_SOURCE: usize = 16 * 1024;
pub const MAX_PIXELS: usize = 1_000_000;

/// Reject excessive input and configuration before starting a helper or parser.
pub fn admissible(source: &str) -> bool {
    if source.is_empty()
        || !source.is_ascii()
        || source.len() > MAX_SOURCE
        || source.lines().count() > 256
        || source.bytes().filter(|b| *b == b';').count() > 256
        || source.bytes().filter(|b| *b == b'&').count() > 64
        || source.contains("%%{")
        || source.trim_start().starts_with("---")
        || source
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return false;
    }
    let header = source
        .lines()
        .map(str::trim)
        .find(|s| !s.is_empty() && !s.starts_with("%%"))
        .and_then(|s| s.split(|c: char| c.is_whitespace() || c == ';').next())
        .unwrap_or_default();
    if !matches!(
        header,
        "flowchart"
            | "graph"
            | "sequenceDiagram"
            | "classDiagram"
            | "stateDiagram"
            | "stateDiagram-v2"
            | "erDiagram"
    ) {
        return false;
    }
    // A conservative lexical nesting guard bounds work before native parsing.
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for character in source.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if character == '"' {
            quoted = !quoted;
            continue;
        }
        if quoted {
            continue;
        }
        if matches!(character, '[' | '(' | '{') {
            depth += 1;
            if depth > 32 {
                return false;
            }
        } else if matches!(character, ']' | ')' | '}') {
            depth = depth.saturating_sub(1);
        }
    }
    !matches!(header, "flowchart" | "graph") || (depth == 0 && !quoted)
}

pub fn render(source: &str, light: bool) -> Option<Preview> {
    use mermaid_rs_renderer::{
        DiagramKind, LayoutConfig, Theme, compute_layout, parse_mermaid_strict, render_svg,
    };
    if !admissible(source) {
        return None;
    }
    let parsed = parse_mermaid_strict(source).ok()?;
    let graph = &parsed.graph;
    // Attribute tables in this backend require OS-font metrics and can omit
    // types when those metrics are unavailable. Preserve the full code instead.
    if graph.kind == DiagramKind::Er && graph.nodes.values().any(|n| n.label.contains('\n')) {
        return None;
    }
    if parsed.init_config.is_some()
        || !matches!(
            graph.kind,
            DiagramKind::Flowchart
                | DiagramKind::Sequence
                | DiagramKind::Class
                | DiagramKind::State
                | DiagramKind::Er
        )
        || graph.nodes.len() > 128
        || graph.edges.len() > 256
        || graph.subgraphs.len() > 16
        || graph.subgraphs.iter().map(|g| g.nodes.len()).sum::<usize>() > 512
        || graph.sequence_participants.len() > 32
        || graph.sequence_frames.len() > 16
        || graph.sequence_notes.len() > 64
        || graph.sequence_activations.len() > 128
        || graph.sequence_boxes.len() > 16
        || graph.state_notes.len() > 64
        || !graph.node_links.is_empty()
        || graph
            .nodes
            .values()
            .any(|node| node.icon.is_some() || node.label.len() > 1024 || !node.label.is_ascii())
    {
        return None;
    }
    let mut theme = if light {
        Theme::modern()
    } else {
        Theme::dark()
    };
    if !light {
        theme.background = "#252a33".into();
        theme.primary_color = "#243447".into();
        theme.primary_border_color = "#63cce3".into();
        theme.primary_text_color = "#e3eaf2".into();
        theme.text_color = "#e3eaf2".into();
        theme.line_color = "#b1c3d4".into();
        theme.edge_label_background = theme.background.clone();
        theme.sequence_actor_fill = theme.primary_color.clone();
        theme.sequence_actor_border = theme.primary_border_color.clone();
    }
    theme.font_family = "KaTeX_SansSerif".into();
    theme.font_size = 16.;
    let config = LayoutConfig {
        fast_text_metrics: true,
        ..Default::default()
    };
    let layout = compute_layout(graph, &theme, &config);
    let svg = render_svg(&layout, &theme, &config);
    svg::rasterize(&svg).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn supported_diagrams_have_bounded_retina_rasters_in_both_themes() {
        for source in [
            "flowchart LR\n A[Start] --> B{Ready?}\n B -->|Yes| C[Done]\n",
            "sequenceDiagram\n Alice->>Bob: Hello\n Bob-->>Alice: Reply\n",
            "classDiagram\n class Note {\n +String title\n }\n Note --> Reader\n",
            "stateDiagram-v2\n [*] --> Ready\n Ready --> [*]\n",
            "erDiagram\n BOOK ||--o{ PAGE : contains\n",
            "flowchart TD\n subgraph Input\n A-->B\n end\n B-->C\n",
        ] {
            for light in [false, true] {
                let p = render(source, light).unwrap_or_else(|| panic!("Failed: {source}"));
                assert!(p.width > 0 && p.height > 0);
                assert!(p.width as usize * p.height as usize <= MAX_PIXELS);
                assert_eq!(p.rgba.len(), (p.width * p.height * 4) as usize);
                assert!(p.rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
                assert!(p.width <= p.original_width * 2 && p.height <= p.original_height * 2);
            }
        }
    }
    #[test]
    fn unsupported_excessive_or_external_content_keeps_source() {
        for source in [
            "pie\n\"A\":1\n",
            "%%{init: {}}%%\nflowchart LR\nA-->B\n",
            "---\ntheme: dark\n---\nflowchart LR; A-->B",
            "flowchart LR\nA\x1b[2J-->B",
            "flowchart LR\nA[中文]-->B",
            "flowchart LR\nA[Broken",
            "erDiagram\n BOOK {\n string title\n }\n",
            "flowchart LR\nA-->B\nclick A \"https://example.com\"\n",
        ] {
            assert!(render(source, false).is_none(), "Accepted: {source}");
        }
        assert!(!admissible(&format!(
            "flowchart LR\n{}",
            "A".repeat(MAX_SOURCE)
        )));
        assert!(!admissible(&format!("flowchart LR\n{}", "A;".repeat(257))));
        assert!(!admissible(&format!(
            "flowchart LR\nA{}B{}",
            "[".repeat(33),
            "]".repeat(33)
        )));
        let large = format!(
            "flowchart LR\n{}",
            (0..129)
                .map(|i| format!("N{i}[Node {i}]\n"))
                .collect::<String>()
        );
        assert!(render(&large, false).is_none());
        assert!(render("flowchart LR\nA-->B", false).is_some());
    }
}
