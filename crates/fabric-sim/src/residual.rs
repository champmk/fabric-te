//! CIR leftover. Transcribed from docs/DESIGN.md §11.1–§11.2.

use fabric_topo::Graph;
use fabric_types::LinkId;

pub struct Residual {
    pub cir: Vec<u64>,
    pub r_avail: Vec<u64>,
    pub q_bytes: Vec<u64>,
    pub overflowed: Vec<bool>,
}

impl Residual {
    pub fn new(graph: &Graph) -> Self {
        let n = graph.links.len();
        let mut r = Self {
            cir: vec![0; n],
            r_avail: vec![0; n],
            q_bytes: vec![0; n],
            overflowed: vec![false; n],
        };
        r.recompute(graph);
        r
    }

    pub fn admissible(graph: &Graph, e: LinkId) -> u64 {
        let Some(link) = graph.link(e) else {
            return 0;
        };
        if link.failed {
            0
        } else {
            link.capacity_Bps * 95 / 100
        }
    }

    pub fn recompute(&mut self, graph: &Graph) {
        let n = graph.links.len();
        self.cir.resize(n, 0);
        self.r_avail.resize(n, 0);
        self.q_bytes.resize(n, 0);
        self.overflowed.resize(n, false);
        for link in &graph.links {
            let i = link.id.0 as usize;
            let adm = Self::admissible(graph, link.id);
            self.r_avail[i] = adm.saturating_sub(self.cir[i]);
        }
    }

    /// Example C fixture. No GPU occupy.
    pub fn inject_cir(&mut self, graph: &Graph, e: LinkId, rho: u64) {
        let i = e.0 as usize;
        if i >= self.cir.len() {
            return;
        }
        self.cir[i] = self.cir[i].saturating_add(rho);
        let adm = Self::admissible(graph, e);
        self.r_avail[i] = adm.saturating_sub(self.cir[i]);
    }

    pub fn r_avail(&self, e: LinkId) -> u64 {
        self.r_avail.get(e.0 as usize).copied().unwrap_or(0)
    }

    /// Leftover `c_e − cir` (scratch open). Failed link → 0. §12.4
    pub fn physical_leftover(&self, graph: &Graph) -> Vec<u64> {
        graph
            .links
            .iter()
            .map(|link| {
                let cir = self.cir.get(link.id.0 as usize).copied().unwrap_or(0);
                if link.failed {
                    0
                } else {
                    link.capacity_Bps.saturating_sub(cir)
                }
            })
            .collect()
    }

    /// Instant leftover `c_e` (no CIR). Failed → 0.
    pub fn capacity_leftover(&self, graph: &Graph) -> Vec<u64> {
        graph
            .links
            .iter()
            .map(|link| if link.failed { 0 } else { link.capacity_Bps })
            .collect()
    }

    pub fn release_cir(&mut self, graph: &Graph, e: LinkId, rho: u64) {
        let i = e.0 as usize;
        if i >= self.cir.len() {
            return;
        }
        self.cir[i] = self.cir[i].saturating_sub(rho);
        let adm = Self::admissible(graph, e);
        self.r_avail[i] = adm.saturating_sub(self.cir[i]);
    }

    pub fn clear_cir(&mut self, graph: &Graph) {
        self.cir.fill(0);
        self.recompute(graph);
    }

    /// Cost_e = 1 / (r_avail + ε), ε = 1e-12 * c_e.
    pub fn cost(&self, graph: &Graph, e: LinkId) -> f64 {
        let c = graph.link(e).map(|l| l.capacity_Bps).unwrap_or(0);
        let eps = 1e-12 * (c as f64);
        1.0 / (self.r_avail(e) as f64 + eps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_topo::Graph;

    #[test]
    fn residual_admissible_integer() {
        let g = Graph::generate(256, 8, 1).expect("n32");
        let e = g.host_up(fabric_types::GpuId(0)).expect("hs");
        let c = g.link(e).expect("link").capacity_Bps;
        assert_eq!(c, 50_000_000_000);
        assert_eq!(c * 95 / 100, 47_500_000_000);
        assert_eq!(c - c * 95 / 100, 2_500_000_000);
        assert_eq!(Residual::admissible(&g, e), 47_500_000_000);
        let res = Residual::new(&g);
        assert_eq!(res.r_avail(e), 47_500_000_000);
    }
}
