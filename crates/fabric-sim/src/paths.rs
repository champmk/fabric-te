//! Clos k-shortest. Transcribed from docs/DESIGN.md §11.3. No Yen.

use fabric_topo::Graph;
use fabric_types::{GpuId, LinkId, SpineId};

use crate::residual::Residual;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Path {
    pub links: Vec<LinkId>,
}

impl Path {
    pub fn empty() -> Self {
        Self { links: Vec::new() }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum PathMode {
    Joint,
    Ecmp,
}

pub fn k_shortest(
    src: GpuId,
    dst: GpuId,
    g: &Graph,
    res: &Residual,
    k: usize,
    mode: PathMode,
) -> Vec<Path> {
    if k == 0 {
        return Vec::new();
    }
    let (Some(sn), Some(dn)) = (g.gpu(src), g.gpu(dst)) else {
        return Vec::new();
    };
    if sn.node == dn.node {
        return vec![Path::empty()];
    }
    let (Some(sl), Some(dl)) = (g.leaf_of(src), g.leaf_of(dst)) else {
        return Vec::new();
    };
    let (Some(hs), Some(hd)) = (g.host_up(src), g.host_down(dst)) else {
        return Vec::new();
    };
    if g.link(hs).is_some_and(|l| l.failed) || g.link(hd).is_some_and(|l| l.failed) {
        return Vec::new();
    }
    if sl == dl {
        let path = Path {
            links: vec![hs, hd],
        };
        if mode == PathMode::Joint && path.links.iter().any(|&e| res.r_avail(e) == 0) {
            return Vec::new();
        }
        return vec![path];
    }

    let mut cands: Vec<(f64, u32, u32, u32, Path)> = Vec::new();
    for spine in g.common_spines(sl, dl) {
        let ups = g.ups_to_spine(sl, spine);
        let downs = g.downs_from_spine(spine, dl);
        let n = ups.len().min(downs.len());
        for i in 0..n {
            let u = ups[i];
            let d = downs[i];
            if [hs, u, d, hd]
                .iter()
                .any(|&id| g.link(id).is_some_and(|l| l.failed))
            {
                continue;
            }
            let path = Path {
                links: vec![hs, u, d, hd],
            };
            if mode == PathMode::Joint && path.links.iter().any(|&e| res.r_avail(e) == 0) {
                continue;
            }
            let cost = match mode {
                PathMode::Joint => path.links.iter().map(|&e| res.cost(g, e)).sum::<f64>(),
                PathMode::Ecmp => path.links.len() as f64,
            };
            cands.push((cost, spine.0, u.0, d.0, path));
        }
    }
    cands.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
            .then(a.3.cmp(&b.3))
    });
    cands.into_iter().take(k).map(|(_, _, _, _, p)| p).collect()
}

pub fn hops(p: &Path) -> usize {
    p.links.len()
}

pub fn first_spine(g: &Graph, p: &Path) -> Option<SpineId> {
    p.links.iter().find_map(|&id| {
        g.link(id).and_then(|l| match l.dst {
            fabric_types::Endpoint::Spine(s) => Some(s),
            _ => None,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Residual;
    use fabric_topo::Graph;
    use fabric_types::GpuId;

    fn n32() -> Graph {
        Graph::generate(256, 8, 1).expect("n32")
    }

    #[test]
    fn joint_kshortest_k8() {
        let g = n32();
        let res = Residual::new(&g);
        // Different nodes, different rails → 4-hop via spine.
        let paths = k_shortest(GpuId(0), GpuId(9), &g, &res, 8, PathMode::Joint);
        assert!(!paths.is_empty());
        assert!(paths.len() <= 8);
        assert_eq!(paths.len(), 8);
        assert!(paths.iter().all(|p| p.links.len() == 4));
    }

    #[test]
    fn joint_cost_inverse_residual() {
        let g = n32();
        let mut res = Residual::new(&g);
        let e0 = g.host_up(GpuId(0)).expect("hs");
        let e1 = g.host_up(GpuId(1)).expect("hs");
        let c0 = res.cost(&g, e0);
        res.inject_cir(&g, e0, Residual::admissible(&g, e0) / 2);
        let c0_hot = res.cost(&g, e0);
        let c1 = res.cost(&g, e1);
        assert!(c0_hot > c0);
        assert!(c0_hot > c1);
        assert!(res.r_avail(e0) < res.r_avail(e1));
    }

    #[test]
    fn naive_ecmp_tiebreak_lowest_linkid() {
        let g = n32();
        let res = Residual::new(&g);
        let paths = k_shortest(GpuId(0), GpuId(9), &g, &res, 8, PathMode::Ecmp);
        assert_eq!(paths.len(), 8);
        let first = &paths[0];
        let spine = first_spine(&g, first).expect("spine");
        assert_eq!(spine.0, 0);
        // Among spine-0 zips, lowest (up, down) LinkId pair.
        let sl = g.leaf_of(GpuId(0)).unwrap();
        let dl = g.leaf_of(GpuId(9)).unwrap();
        let ups = g.ups_to_spine(sl, spine);
        let downs = g.downs_from_spine(spine, dl);
        assert_eq!(first.links[1], ups[0]);
        assert_eq!(first.links[2], downs[0]);
        let hops0 = hops(first);
        assert!(paths.iter().all(|p| hops(p) == hops0));
    }

    #[test]
    fn same_node_is_empty_path() {
        let g = n32();
        let res = Residual::new(&g);
        let paths = k_shortest(GpuId(0), GpuId(1), &g, &res, 8, PathMode::Joint);
        assert_eq!(paths, vec![Path::empty()]);
    }

    #[test]
    fn same_leaf_is_two_hops() {
        let g = n32();
        let res = Residual::new(&g);
        // Gpu 0 = n0r0, Gpu 8 = n1r0. Same rail, same leaf group.
        let paths = k_shortest(GpuId(0), GpuId(8), &g, &res, 8, PathMode::Joint);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].links.len(), 2);
        assert_eq!(paths[0].links[0], g.host_up(GpuId(0)).unwrap());
        assert_eq!(paths[0].links[1], g.host_down(GpuId(8)).unwrap());
    }

    #[test]
    fn joint_skips_zero_leftover() {
        let g = n32();
        let mut res = Residual::new(&g);
        let hs = g.host_up(GpuId(0)).expect("hs");
        res.inject_cir(&g, hs, Residual::admissible(&g, hs));
        assert_eq!(res.r_avail(hs), 0);
        let joint = k_shortest(GpuId(0), GpuId(9), &g, &res, 8, PathMode::Joint);
        assert!(joint.is_empty());
        let same_leaf = k_shortest(GpuId(0), GpuId(8), &g, &res, 8, PathMode::Joint);
        assert!(same_leaf.is_empty());
    }

    #[test]
    fn naive_ecmp_still_routes_zero_leftover() {
        let g = n32();
        let mut res = Residual::new(&g);
        let hs = g.host_up(GpuId(0)).expect("hs");
        res.inject_cir(&g, hs, Residual::admissible(&g, hs));
        let paths = k_shortest(GpuId(0), GpuId(9), &g, &res, 8, PathMode::Ecmp);
        assert_eq!(paths.len(), 8);
        assert!(paths.iter().all(|p| p.links[0] == hs));
    }
}
