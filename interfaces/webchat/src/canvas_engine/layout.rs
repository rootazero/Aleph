use super::types::{CanvasEdge, CanvasNode, Vec2};

pub struct LayoutConfig {
    pub repulsion_strength: f64,
    pub attraction_strength: f64,
    pub damping: f64,
    pub center_gravity: f64,
    pub max_velocity: f64,
    pub convergence_threshold: f64,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            repulsion_strength: 800.0,
            attraction_strength: 0.015,
            damping: 0.85,
            center_gravity: 0.02,
            max_velocity: 40.0,
            convergence_threshold: 0.5,
        }
    }
}

pub struct ForceLayout {
    pub config: LayoutConfig,
    pub is_settled: bool,
}

impl ForceLayout {
    pub fn new() -> Self {
        Self {
            config: LayoutConfig::default(),
            is_settled: false,
        }
    }

    pub fn tick(&mut self, nodes: &mut [CanvasNode], edges: &[CanvasEdge]) -> f64 {
        let n = nodes.len();
        if n == 0 {
            self.is_settled = true;
            return 0.0;
        }

        let mut forces = vec![Vec2::zero(); n];

        // Repulsion between all pairs of nodes
        for i in 0..n {
            for j in (i + 1)..n {
                let delta = nodes[i].position - nodes[j].position;
                let dist = delta.length().max(1.0);
                let force = delta.normalized() * (self.config.repulsion_strength / (dist * dist));
                forces[i] += force;
                forces[j] = forces[j] - force;
            }
        }

        // Attraction along edges
        for edge in edges {
            if edge.from_idx >= n || edge.to_idx >= n {
                continue;
            }
            let delta = nodes[edge.to_idx].position - nodes[edge.from_idx].position;
            let dist = delta.length().max(1.0);
            let force = delta.normalized() * (dist * self.config.attraction_strength);
            forces[edge.from_idx] += force;
            forces[edge.to_idx] = forces[edge.to_idx] - force;
        }

        // Center gravity pulls all nodes toward the origin
        for i in 0..n {
            forces[i] += (Vec2::zero() - nodes[i].position) * self.config.center_gravity;
        }

        // Apply forces and integrate velocity
        let mut total_energy = 0.0;
        for i in 0..n {
            if nodes[i].pinned {
                nodes[i].velocity = Vec2::zero();
                continue;
            }
            nodes[i].velocity = (nodes[i].velocity + forces[i]) * self.config.damping;
            let speed = nodes[i].velocity.length();
            if speed > self.config.max_velocity {
                nodes[i].velocity = nodes[i].velocity.normalized() * self.config.max_velocity;
            }
            nodes[i].position += nodes[i].velocity;
            total_energy += speed * speed;
        }

        self.is_settled = total_energy < self.config.convergence_threshold;
        total_energy
    }

    pub fn wake(&mut self) {
        self.is_settled = false;
    }
}

impl Default for ForceLayout {
    fn default() -> Self {
        Self::new()
    }
}
