use web_sys::CanvasRenderingContext2d;

pub const MINIMAP_W: f32 = 160.0;
pub const MINIMAP_H: f32 = 120.0;
pub const MINIMAP_SAMPLE_LIMIT: usize = 200;
pub const MINIMAP_TTL_MS: f64 = 5.0 * 60_000.0;

pub struct MiniMap {
    pub samples: Vec<MiniNode>,
    pub bounds: (f32, f32, f32, f32), // (min_x, min_y, max_x, max_y)
    pub fetched_at_ms: f64,
}

pub struct MiniNode {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub kind: String,
}

impl MiniMap {
    pub fn empty() -> Self {
        Self {
            samples: vec![],
            bounds: (0.0, 0.0, 1.0, 1.0),
            fetched_at_ms: 0.0,
        }
    }

    pub fn is_stale(&self, now_ms: f64) -> bool {
        now_ms - self.fetched_at_ms > MINIMAP_TTL_MS
    }

    /// Set samples and recompute bounds.
    pub fn set_samples(&mut self, samples: Vec<MiniNode>, now_ms: f64) {
        if samples.is_empty() {
            self.samples = samples;
            self.bounds = (0.0, 0.0, 1.0, 1.0);
            self.fetched_at_ms = now_ms;
            return;
        }
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for s in &samples {
            min_x = min_x.min(s.x);
            min_y = min_y.min(s.y);
            max_x = max_x.max(s.x);
            max_y = max_y.max(s.y);
        }
        if (max_x - min_x).abs() < 1e-3 {
            max_x = min_x + 1.0;
        }
        if (max_y - min_y).abs() < 1e-3 {
            max_y = min_y + 1.0;
        }
        self.samples = samples;
        self.bounds = (min_x, min_y, max_x, max_y);
        self.fetched_at_ms = now_ms;
    }

    pub fn world_to_minimap(&self, wx: f32, wy: f32) -> (f32, f32) {
        let (min_x, min_y, max_x, max_y) = self.bounds;
        let nx = (wx - min_x) / (max_x - min_x);
        let ny = (wy - min_y) / (max_y - min_y);
        (nx * MINIMAP_W, ny * MINIMAP_H)
    }

    pub fn minimap_to_world(&self, mx: f32, my: f32) -> (f32, f32) {
        let (min_x, min_y, max_x, max_y) = self.bounds;
        let wx = min_x + (mx / MINIMAP_W) * (max_x - min_x);
        let wy = min_y + (my / MINIMAP_H) * (max_y - min_y);
        (wx, wy)
    }

    pub fn pick_node(&self, mx: f32, my: f32) -> Option<&MiniNode> {
        let mut best: Option<(&MiniNode, f32)> = None;
        for s in &self.samples {
            let (sx, sy) = self.world_to_minimap(s.x, s.y);
            let d2 = (sx - mx).powi(2) + (sy - my).powi(2);
            if d2 < 36.0 {
                match best {
                    Some((_, bd)) if bd <= d2 => {}
                    _ => best = Some((s, d2)),
                }
            }
        }
        best.map(|(n, _)| n)
    }

    pub fn draw(
        &self,
        ctx: &CanvasRenderingContext2d,
        x_origin: f32,
        y_origin: f32,
        active_id: &str,
        one_hop_ids: &std::collections::HashSet<String>,
    ) {
        // Background panel
        ctx.set_fill_style_str("rgba(20,20,30,0.7)");
        ctx.fill_rect(
            x_origin as f64,
            y_origin as f64,
            MINIMAP_W as f64,
            MINIMAP_H as f64,
        );
        ctx.set_stroke_style_str("rgba(255,255,255,0.15)");
        ctx.set_line_width(1.0);
        ctx.stroke_rect(
            x_origin as f64,
            y_origin as f64,
            MINIMAP_W as f64,
            MINIMAP_H as f64,
        );

        for s in &self.samples {
            let (mx, my) = self.world_to_minimap(s.x, s.y);
            let cx = x_origin + mx;
            let cy = y_origin + my;
            if s.id == active_id {
                ctx.set_fill_style_str("#ef4444");
                ctx.begin_path();
                let _ = ctx.arc(cx as f64, cy as f64, 4.0, 0.0, std::f64::consts::TAU);
                ctx.fill();
                ctx.set_stroke_style_str("#ffffff");
                ctx.set_line_width(1.0);
                ctx.stroke();
            } else if one_hop_ids.contains(&s.id) {
                ctx.set_fill_style_str("rgba(167,139,250,0.85)");
                ctx.fill_rect((cx - 1.5) as f64, (cy - 1.5) as f64, 3.0, 3.0);
            } else {
                ctx.set_fill_style_str("rgba(140,140,160,0.5)");
                ctx.fill_rect((cx - 0.5) as f64, (cy - 0.5) as f64, 1.0, 1.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_bounds_safe() {
        let mut m = MiniMap::empty();
        m.set_samples(vec![], 0.0);
        let (x, y) = m.world_to_minimap(0.0, 0.0);
        assert!(x.is_finite() && y.is_finite());
    }

    #[test]
    fn world_to_minimap_round_trip() {
        let mut m = MiniMap::empty();
        m.set_samples(
            vec![
                MiniNode {
                    id: "a".to_string(),
                    x: -100.0,
                    y: -100.0,
                    kind: "concept".to_string(),
                },
                MiniNode {
                    id: "b".to_string(),
                    x: 100.0,
                    y: 100.0,
                    kind: "concept".to_string(),
                },
            ],
            0.0,
        );
        let (mx, my) = m.world_to_minimap(0.0, 0.0);
        let (wx, wy) = m.minimap_to_world(mx, my);
        assert!((wx).abs() < 1e-3);
        assert!((wy).abs() < 1e-3);
    }

    #[test]
    fn pick_node_finds_closest_within_threshold() {
        let mut m = MiniMap::empty();
        m.set_samples(
            vec![MiniNode {
                id: "a".to_string(),
                x: 0.0,
                y: 0.0,
                kind: "concept".to_string(),
            }],
            0.0,
        );
        let (mx, my) = m.world_to_minimap(0.0, 0.0);
        assert!(m.pick_node(mx, my).is_some());
        assert!(m.pick_node(mx + 100.0, my + 100.0).is_none());
    }
}
