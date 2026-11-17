//! Live force simulation for the graph view, modelled on d3-force (which Obsidian's graph
//! also follows): link springs, many-body repulsion, a weak pull to the centre, and an
//! `alpha` that cools so the layout settles and stops costing frames.

const LINK_DISTANCE: f32 = 46.0;
const REPULSION: f32 = -110.0;
const GRAVITY: f32 = 0.035;
const VELOCITY_DECAY: f32 = 0.42;
const ALPHA_MIN: f32 = 0.002;
const ALPHA_DECAY: f32 = 0.022;
/// Pairs further apart than this barely push each other; skipping them keeps big graphs cheap.
const REPULSION_RANGE: f32 = 520.0;

#[derive(Clone, Debug)]
pub struct SimNode {
    pub x: f32,
    pub y: f32,
    vx: f32,
    vy: f32,
    /// Held under the pointer while dragged.
    pub fixed: Option<(f32, f32)>,
}

pub struct Simulation {
    pub nodes: Vec<SimNode>,
    pub links: Vec<(usize, usize)>,
    pub degree: Vec<usize>,
    alpha: f32,
    alpha_target: f32,
}

impl Simulation {
    pub fn new(count: usize, links: Vec<(usize, usize)>) -> Self {
        let mut degree = vec![0usize; count];
        for &(a, b) in &links {
            degree[a] += 1;
            degree[b] += 1;
        }
        // Phyllotaxis start, as d3 does: even spread, deterministic.
        let nodes = (0..count)
            .map(|i| {
                let radius = 10.0 * (0.5 + i as f32).sqrt();
                let angle = i as f32 * std::f32::consts::PI * (3.0 - 5f32.sqrt());
                SimNode { x: radius * angle.cos(), y: radius * angle.sin(), vx: 0.0, vy: 0.0, fixed: None }
            })
            .collect();
        Self { nodes, links, degree, alpha: 1.0, alpha_target: 0.0 }
    }

    pub fn is_running(&self) -> bool {
        self.alpha >= ALPHA_MIN || self.alpha_target > 0.0
    }

    /// Starts a rebuilt-but-familiar layout cooler than a fresh one, so it only settles locally.
    pub fn set_alpha(&mut self, alpha: f32) {
        self.alpha = alpha;
    }

    /// Keeps the layout warm while a node is held, like d3's `alphaTarget`.
    pub fn set_alpha_target(&mut self, target: f32) {
        self.alpha_target = target;
        if target > 0.0 {
            self.alpha = self.alpha.max(target);
        }
    }

    pub fn tick(&mut self) {
        self.alpha += (self.alpha_target - self.alpha) * ALPHA_DECAY;
        let alpha = self.alpha;
        let n = self.nodes.len();
        if n == 0 {
            return;
        }

        for &(a, b) in &self.links {
            let (sa, sb) = (&self.nodes[a], &self.nodes[b]);
            let mut dx = sb.x + sb.vx - sa.x - sa.vx;
            let mut dy = sb.y + sb.vy - sa.y - sa.vy;
            if dx == 0.0 && dy == 0.0 {
                dx = 1e-3;
                dy = 1e-3;
            }
            let len = (dx * dx + dy * dy).sqrt();
            let strength = 1.0 / self.degree[a].min(self.degree[b]).max(1) as f32;
            let k = (len - LINK_DISTANCE) / len * alpha * strength;
            dx *= k;
            dy *= k;
            let (da, db) = (self.degree[a] as f32, self.degree[b] as f32);
            let bias = da / (da + db).max(1.0);
            self.nodes[b].vx -= dx * bias;
            self.nodes[b].vy -= dy * bias;
            self.nodes[a].vx += dx * (1.0 - bias);
            self.nodes[a].vy += dy * (1.0 - bias);
        }

        let range2 = REPULSION_RANGE * REPULSION_RANGE;
        for i in 0..n {
            let (xi, yi) = (self.nodes[i].x, self.nodes[i].y);
            let (mut fx, mut fy) = (0.0f32, 0.0f32);
            for j in 0..n {
                if i == j {
                    continue;
                }
                let dx = self.nodes[j].x - xi;
                let dy = self.nodes[j].y - yi;
                let d2 = dx * dx + dy * dy;
                if d2 > range2 {
                    continue;
                }
                let d2 = d2.max(1.0);
                let w = REPULSION * alpha / d2;
                fx += dx * w;
                fy += dy * w;
            }
            let node = &mut self.nodes[i];
            node.vx += fx - node.x * GRAVITY * alpha;
            node.vy += fy - node.y * GRAVITY * alpha;
        }

        for node in &mut self.nodes {
            if let Some((x, y)) = node.fixed {
                node.x = x;
                node.y = y;
                node.vx = 0.0;
                node.vy = 0.0;
                continue;
            }
            node.vx *= 1.0 - VELOCITY_DECAY;
            node.vy *= 1.0 - VELOCITY_DECAY;
            node.x += node.vx;
            node.y += node.vy;
        }
    }

    /// World-space bounding box `(min_x, min_y, max_x, max_y)`.
    pub fn extent(&self) -> Option<(f32, f32, f32, f32)> {
        if self.nodes.is_empty() {
            return None;
        }
        Some(self.nodes.iter().fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |(a, b, c, d), n| {
            (a.min(n.x), b.min(n.y), c.max(n.x), d.max(n.y))
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linked_nodes_end_up_closer_and_layout_settles() {
        let mut sim = Simulation::new(8, vec![(0, 1), (1, 2), (2, 0)]);
        let mut ticks = 0;
        while sim.is_running() && ticks < 2000 {
            sim.tick();
            ticks += 1;
        }
        assert!(!sim.is_running(), "simulation should cool down");
        let d = |a: usize, b: usize| ((sim.nodes[a].x - sim.nodes[b].x).powi(2) + (sim.nodes[a].y - sim.nodes[b].y).powi(2)).sqrt();
        assert!(d(0, 1) < d(0, 5));
        assert!(sim.nodes.iter().all(|n| n.x.is_finite() && n.y.is_finite()));
    }

    #[test]
    fn fixed_node_stays_put() {
        let mut sim = Simulation::new(3, vec![(0, 1)]);
        sim.nodes[0].fixed = Some((100.0, -50.0));
        sim.tick();
        assert_eq!((sim.nodes[0].x, sim.nodes[0].y), (100.0, -50.0));
    }
}
