//! Pure graph indexing and layout shared by both Panel renderers.

mod fnv1a;
mod layout;
mod vec3;

use std::collections::{HashMap, HashSet};

use aleph_protocol::memory_graph::{NoteLinkDto, NoteNodeDto};

pub use fnv1a::fnv1a_32;
pub use layout::ForceLayout;
pub use vec3::Vec3;

pub struct Relation<'a> {
    pub from: u32,
    pub to: u32,
    pub row: &'a NoteLinkDto,
}

pub struct NebulaIndex<'a> {
    pub relations: Vec<Relation<'a>>,
    pub springs: Vec<(u32, u32)>,
}

impl<'a> NebulaIndex<'a> {
    pub fn new(nodes: &[NoteNodeDto], edges: &'a [NoteLinkDto]) -> Self {
        let mut indices = HashMap::with_capacity(nodes.len());
        for (index, node) in nodes.iter().enumerate() {
            indices.entry(node.id.as_str()).or_insert(index as u32);
        }
        let mut relations = Vec::new();
        let mut springs = Vec::new();
        let mut seen = HashSet::new();
        for row in edges {
            let (Some(&from), Some(&to)) =
                (indices.get(row.from.as_str()), indices.get(row.to.as_str()))
            else {
                continue;
            };
            relations.push(Relation { from, to, row });
            let pair = (from.min(to), from.max(to));
            if from != to && seen.insert(pair) {
                springs.push(pair);
            }
        }
        Self { relations, springs }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relations_remain_directed_while_springs_coalesce() {
        let node = |id: &str| NoteNodeDto {
            id: id.into(),
            name: id.into(),
            path: id.into(),
            agent_id: None,
            category: "wiki".into(),
            tags: Vec::new(),
            link_count: 0,
            community_id: None,
            updated_at: None,
        };
        let edge = |from: &str, to: &str, kind: &str, confidence: Option<f32>| NoteLinkDto {
            from: from.into(),
            to: to.into(),
            label: None,
            kind: Some(kind.into()),
            confidence,
        };
        let nodes = [node("a"), node("b"), node("c")];
        let edges = [
            edge("a", "c", "related_similarity", None),
            edge("a", "b", "refers", Some(1.0)),
            edge("b", "a", "derives", Some(0.8)),
            edge("a", "unavailable", "refers", Some(1.0)),
        ];
        let index = NebulaIndex::new(&nodes, &edges);
        let directed: Vec<_> = index
            .relations
            .iter()
            .map(|edge| (edge.from, edge.to))
            .collect();
        assert_eq!(directed, vec![(0, 2), (0, 1), (1, 0)]);
        assert_eq!(index.springs, vec![(0, 2), (0, 1)]);
        assert_eq!(index.relations[0].row.confidence, None);
    }
}
