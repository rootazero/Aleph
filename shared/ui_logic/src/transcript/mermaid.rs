//! Layout a bounded subset of Mermaid flowcharts without HTML or a JS engine.

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub struct FlowchartScene {
    pub width: u32,
    pub height: u32,
    pub left_to_right: bool,
    pub nodes: Vec<FlowchartNode>,
    pub edges: Vec<FlowchartEdge>,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub struct FlowchartNode {
    pub label: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub struct FlowchartEdge {
    pub from: usize,
    pub to: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FlowchartFallback {
    SourceLimit,
    NodeLimit,
    EdgeLimit,
    LabelLimit,
    CanvasLimit,
    Unsupported,
    Unsafe,
    Cycle,
}

use FlowchartFallback as F;

fn ws(input: &str) -> &str {
    input.trim_start_matches([' ', '\t'])
}

fn word(input: &str) -> (&str, &str) {
    let end = input
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    input.split_at(end)
}

fn node<'a>(input: &mut &'a str, nodes: &mut Vec<(&'a str, Option<String>)>) -> Result<usize, F> {
    let (id, rest) = word(ws(input));
    if id.is_empty()
        || matches!(
            id,
            "end"
                | "subgraph"
                | "direction"
                | "class"
                | "classDef"
                | "style"
                | "linkStyle"
                | "flowchart"
                | "graph"
                | "accTitle"
                | "accDescr"
        )
    {
        return Err(F::Unsupported);
    }
    *input = ws(rest);
    let label = if let Some(rest) = input.strip_prefix('[') {
        let (text, rest) = rest.split_once(']').ok_or(F::Unsupported)?;
        *input = ws(rest);
        let text = text.trim();
        let text = if let Some(quoted) = text.strip_prefix('"') {
            quoted.strip_suffix('"').ok_or(F::Unsupported)?
        } else {
            text
        };
        if text.is_empty()
            || text.contains(['[', '"'])
            || matches!(text.chars().next(), Some('/' | '\\' | '('))
        {
            return Err(F::Unsupported);
        }
        // Slash/backslash delimit Mermaid's parallelogram and trapezoid shapes.
        if text.ends_with(['/', '\\']) {
            return Err(F::Unsupported);
        }
        if text.chars().count() > 80 {
            return Err(F::LabelLimit);
        }
        Some(text.to_owned())
    } else {
        if id.len() > 80 {
            return Err(F::LabelLimit);
        }
        None
    };
    if let Some(i) = nodes.iter().position(|(key, _)| *key == id) {
        if let Some(label) = label {
            if nodes[i].1.as_ref().is_some_and(|old| *old != label) {
                return Err(F::Unsupported);
            }
            nodes[i].1 = Some(label);
        }
        return Ok(i);
    }
    if nodes.len() == 48 {
        return Err(F::NodeLimit);
    }
    nodes.push((id, label));
    Ok(nodes.len() - 1)
}

pub fn layout_flowchart(source: &str) -> Result<FlowchartScene, FlowchartFallback> {
    if source.len() > 8192 {
        return Err(F::SourceLimit);
    }
    // Intentionally conservative; no decoding, interpolation, or resource loading.
    let lower = source.to_ascii_lowercase();
    if lower
        .split("-->")
        .any(|part| part.contains(['<', '&', '@', '`', ':']))
        || ["%%{", "//", "www.", "!["]
            .iter()
            .any(|s| lower.contains(s))
        || lower
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .any(|s| s == "click")
        || source
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\r' | '\n' | '\t'))
    {
        return Err(F::Unsafe);
    }

    let mut direction = None;
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for mut line in source.lines().map(ws) {
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if direction.is_none() {
            let (kind, rest) = word(line);
            let (dir, rest) = word(ws(rest));
            if !matches!(kind, "flowchart" | "graph") || !matches!(dir, "TD" | "LR") {
                return Err(F::Unsupported);
            }
            direction = Some(dir);
            line = ws(rest);
            if !line.is_empty() && !line.starts_with(';') {
                return Err(F::Unsupported);
            }
        }
        while !line.is_empty() {
            if let Some(rest) = line.strip_prefix(';') {
                line = ws(rest);
                continue;
            }
            let mut from = node(&mut line, &mut nodes)?;
            while let Some(rest) = line.strip_prefix("-->") {
                if edges.len() == 96 {
                    return Err(F::EdgeLimit);
                }
                line = rest;
                let to = node(&mut line, &mut nodes)?;
                edges.push(FlowchartEdge { from, to });
                from = to;
            }
            if !line.is_empty() && !line.starts_with(';') {
                return Err(F::Unsupported);
            }
        }
    }
    if nodes.is_empty() {
        return Err(F::Unsupported);
    }

    let mut incoming = vec![0usize; nodes.len()];
    let mut outgoing = vec![Vec::new(); nodes.len()];
    for edge in &edges {
        incoming[edge.to] += 1;
        outgoing[edge.from].push(edge.to);
    }
    let mut ready: std::collections::VecDeque<_> =
        (0..nodes.len()).filter(|&i| incoming[i] == 0).collect();
    let mut rank = vec![0usize; nodes.len()];
    let mut visited = 0;
    while let Some(i) = ready.pop_front() {
        visited += 1;
        for &j in &outgoing[i] {
            rank[j] = rank[j].max(rank[i] + 1);
            incoming[j] -= 1;
            if incoming[j] == 0 {
                ready.push_back(j);
            }
        }
    }
    if visited != nodes.len() {
        return Err(F::Cycle);
    }

    const PAD: u32 = 24;
    const GAP: u32 = 32;
    const HEIGHT: u32 = 40;
    let lr = direction == Some("LR");
    let ranks = rank.iter().copied().max().unwrap_or(0) + 1;
    let mut main_sizes = vec![0; ranks];
    let mut cross = vec![PAD; ranks];
    let mut positions = Vec::with_capacity(nodes.len());
    for ((id, label), &r) in nodes.into_iter().zip(&rank) {
        let label = label.unwrap_or_else(|| id.to_owned());
        let width = (32 + 16 * label.chars().count() as u32).max(72);
        let (main_size, cross_size) = if lr { (width, HEIGHT) } else { (HEIGHT, width) };
        main_sizes[r] = main_sizes[r].max(main_size);
        let (x, y) = if lr { (0, cross[r]) } else { (cross[r], 0) };
        cross[r] += cross_size + GAP;
        positions.push(FlowchartNode {
            label,
            x,
            y,
            width,
            height: HEIGHT,
        });
    }
    let mut main_end = PAD;
    for size in &mut main_sizes {
        let start = main_end;
        main_end += *size + GAP;
        *size = start;
    }
    for (node, r) in positions.iter_mut().zip(rank) {
        if lr {
            node.x = main_sizes[r];
        } else {
            node.y = main_sizes[r];
        }
    }
    let main = main_end - GAP + PAD;
    let cross = cross.into_iter().max().unwrap_or(PAD) - GAP + PAD;
    let (width, height) = if lr { (main, cross) } else { (cross, main) };
    if width > 4096 || height > 4096 || width.saturating_mul(height) > 1_000_000 {
        return Err(F::CanvasLimit);
    }
    Ok(FlowchartScene {
        width,
        height,
        left_to_right: lr,
        nodes: positions,
        edges,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_td_and_lr() {
        for dir in ["TD", "LR"] {
            let scene = layout_flowchart(&format!(
                "graph {dir}; B[終点]; A[\"Start; here\"]-->C-->B; A-->D-->B; E\n%% comment"
            ))
            .unwrap();
            assert_eq!((scene.nodes.len(), scene.edges.len()), (5, 4));
            assert_eq!(scene.nodes[1].label, "Start; here");
            for edge in &scene.edges {
                let (a, b) = (&scene.nodes[edge.from], &scene.nodes[edge.to]);
                assert!(if dir == "TD" {
                    a.y + a.height < b.y
                } else {
                    a.x + a.width < b.x
                });
            }
            for (i, a) in scene.nodes.iter().enumerate() {
                assert!(a.x + a.width < scene.width && a.y + a.height < scene.height);
                for b in &scene.nodes[..i] {
                    assert!(
                        a.x + a.width < b.x
                            || b.x + b.width < a.x
                            || a.y + a.height < b.y
                            || b.y + b.height < a.y
                    );
                }
            }
        }
    }

    #[test]
    fn shorthand_and_late_label() {
        let scene = layout_flowchart("flowchart LR\nA-->B\nB[Done]\nA --> B\n").unwrap();
        assert_eq!(scene.nodes[1].label, "Done");
        assert_eq!(
            scene.edges,
            vec![
                FlowchartEdge { from: 0, to: 1 },
                FlowchartEdge { from: 0, to: 1 }
            ]
        );
    }

    #[test]
    fn unsafe_input() {
        for line in [
            "%%{init: {}}%%",
            "click A callback",
            "A[<b>Hi</b>]",
            "A[https://example.com]",
            "A[//example.com]",
            "A[&lt;img]",
            "A@{ img: 'somefile' }",
            "A[www.example.com]",
            "A[![alt](local.png)]",
        ] {
            assert_eq!(
                layout_flowchart(&format!("graph TD\n{line}")),
                Err(FlowchartFallback::Unsafe),
                "{line}"
            );
        }
    }

    #[test]
    fn unsupported_or_cyclic_input() {
        for raw in [
            "graph RL; A",
            "sequenceDiagram\nA",
            "graph TD",
            "graph TD; A((Circle))",
            "graph TD; A-.->B",
            "graph TD; A[Bad] B",
            "graph TD; A-->|edge|B",
            "graph TD; subgraph X",
            "graph TD; A[One]; A[Two]",
        ] {
            assert_eq!(
                layout_flowchart(raw),
                Err(FlowchartFallback::Unsupported),
                "{raw}"
            );
        }
        for raw in ["graph TD; A-->A", "graph LR; A-->B-->C-->A; D"] {
            assert_eq!(layout_flowchart(raw), Err(FlowchartFallback::Cycle));
        }
    }

    #[test]
    fn exact_limits() {
        fn at_limit(n: usize, e: usize) -> String {
            let mut source = String::from("graph TD");
            for i in 0..8 {
                source += &format!("; N{i}");
            }
            for i in 8..n {
                source += &format!("; N{}-->N{i}", i - 8);
            }
            for _ in (n - 8)..e {
                source += "; N0-->N8";
            }
            source
        }
        assert!(layout_flowchart(&at_limit(48, 96)).is_ok());
        let unconnected = (0..48).fold(String::from("graph TD"), |scene, i| {
            format!("{scene}; N{i}")
        });
        assert!(layout_flowchart(&unconnected).is_err());
        assert_eq!(
            layout_flowchart(&at_limit(49, 96)),
            Err(FlowchartFallback::NodeLimit)
        );
        assert_eq!(
            layout_flowchart(&at_limit(48, 97)),
            Err(FlowchartFallback::EdgeLimit)
        );
        let source = format!("graph TD; A[{}]", "界".repeat(80));
        let padded = format!("{source:8192}");
        let mut padded = padded[..8192].to_owned();
        assert!(layout_flowchart(&padded).is_ok());
        padded.push(' ');
        assert_eq!(
            layout_flowchart(&padded),
            Err(FlowchartFallback::SourceLimit)
        );
        assert_eq!(
            layout_flowchart(&format!("graph TD; A[{}]", "界".repeat(81))),
            Err(FlowchartFallback::LabelLimit)
        );
    }
}
