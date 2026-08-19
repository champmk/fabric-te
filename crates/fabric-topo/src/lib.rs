//! Rail-optimized 2-tier Clos. Transcribed from docs/DESIGN.md §7 and §9.3.
//! No Residual.

#![forbid(unsafe_code)]
#![allow(non_snake_case)]

use std::fmt;

use fabric_types::{
    Endpoint, EpochId, GpuAvail, GpuId, LeafId, LinkId, NicId, NodeId, RailId, SpineId,
};
use serde::Deserialize;

/// Allowed `--oversub` / TOML `oversub` values (§16.1).
const OVERSUB_OK: [u32; 6] = [1, 2, 4, 8, 16, 32];

const DEFAULT_RAILS: u32 = 8;
const DEFAULT_DOWN: u32 = 32;
const DEFAULT_LEAF_RADIX: u32 = 64;
const DEFAULT_PORT_SPEED_GBPS: u32 = 400;
const DEFAULT_SCRATCH: f64 = 0.05;
const DEFAULT_FILL: f64 = 1.0;
const DEFAULT_BUFFER_BYTES: u64 = 33_554_432;

#[derive(Clone, Debug)]
pub struct Node {
    pub id: NodeId,
    pub gpus: Vec<GpuId>, // length R, local rank order
    pub present: bool,    // false after delay-row
}

#[derive(Clone, Debug)]
pub struct Gpu {
    pub id: GpuId,
    pub node: NodeId,
    pub rail: RailId,
    pub nic: NicId,
    pub avail: GpuAvail, // Present | Unavailable. Never Occupied — that is Occupancy
}

#[derive(Clone, Debug)]
pub struct Leaf {
    pub id: LeafId,
    pub rail: RailId,
    pub group: u32,
    pub failed: bool,
}

#[derive(Clone, Debug)]
pub struct Spine {
    pub id: SpineId,
    pub failed: bool,
}

#[derive(Clone, Debug)]
pub struct Link {
    pub id: LinkId,
    pub src: Endpoint,
    pub dst: Endpoint,
    pub capacity_Bps: u64, // bytes/s (SI). 400 Gbps → 50_000_000_000
    pub scratch: f64,      // 0.05
    pub failed: bool,
    pub bytes_this_epoch: u64, // I2: must be 0 if failed
}

#[derive(Clone, Debug)]
pub struct Graph {
    pub epoch: EpochId,
    pub params: TopoParams,
    pub nodes: Vec<Node>,
    pub gpus: Vec<Gpu>,
    pub leaves: Vec<Leaf>,
    pub spines: Vec<Spine>,
    pub links: Vec<Link>, // index == LinkId.0
}

#[derive(Clone, Debug)]
pub struct TopoParams {
    pub nodes: u32,
    pub gpus_per_node: u32,     // = 8
    pub rails: u32,             // = 8
    pub leaf_radix: u32,        // = 64
    pub down: u32,              // = 32
    pub up: u32,                // = 32 / K_omega
    pub port_speed_gbps: u32,   // = 400
    pub scratch: f64,           // = 0.05
    pub fill: f64,              // = 1.0
    pub allow_cross_rail: bool, // = true
    pub buffer_bytes: u64,      // = 33_554_432
    pub buffer_infinite: bool,  // = false
}

/// Construction / TOML failure. Always CLI exit 2.
#[derive(Debug)]
pub enum TopoError {
    Parse(String),
    Schema(String),
    Illegal(String),
}

impl TopoError {
    pub fn e_code(&self) -> &'static str {
        match self {
            TopoError::Parse(_) => "E_PARSE",
            TopoError::Schema(_) => "E_SCHEMA",
            TopoError::Illegal(_) => "E_TOPO",
        }
    }
}

impl fmt::Display for TopoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TopoError::Parse(m) | TopoError::Schema(m) | TopoError::Illegal(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for TopoError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TopoToml {
    kind: String,
    gpus: u32,
    rails: u32,
    oversub: u32,
    leaf_radix: u32,
    down: u32,
    port_speed_gbps: u32,
    scratch: f64,
    fill: f64,
    allow_cross_rail: bool,
    buffer_bytes: u64,
    buffer_infinite: bool,
}

struct BuildInput {
    g_tot: u32,
    rails: u32,
    oversub: u32,
    leaf_radix: u32,
    down: u32,
    port_speed_gbps: u32,
    scratch: f64,
    fill: f64,
    allow_cross_rail: bool,
    buffer_bytes: u64,
    buffer_infinite: bool,
}

impl Graph {
    /// CLI path: `--gpus G_TOT [--rails R] [--oversub K]`. Defaults G=R=8, D=32, P=64.
    pub fn generate(g_tot: u32, rails: u32, oversub: u32) -> Result<Self, TopoError> {
        Self::build(BuildInput {
            g_tot,
            rails,
            oversub,
            leaf_radix: DEFAULT_LEAF_RADIX,
            down: DEFAULT_DOWN,
            port_speed_gbps: DEFAULT_PORT_SPEED_GBPS,
            scratch: DEFAULT_SCRATCH,
            fill: DEFAULT_FILL,
            allow_cross_rail: true,
            buffer_bytes: DEFAULT_BUFFER_BYTES,
            buffer_infinite: false,
        })
    }

    pub fn from_toml(s: &str) -> Result<Self, TopoError> {
        let t: TopoToml = toml::from_str(s).map_err(|e| {
            let msg = e.to_string();
            if msg.contains("unknown field") {
                TopoError::Schema(msg)
            } else if msg.contains("missing field") {
                TopoError::Schema(msg)
            } else {
                TopoError::Parse(msg)
            }
        })?;
        if t.kind != "rail_clos_2tier" {
            return Err(TopoError::Schema(format!(
                "kind must be rail_clos_2tier, got {}",
                t.kind
            )));
        }
        Self::build(BuildInput {
            g_tot: t.gpus,
            rails: t.rails,
            oversub: t.oversub,
            leaf_radix: t.leaf_radix,
            down: t.down,
            port_speed_gbps: t.port_speed_gbps,
            scratch: t.scratch,
            fill: t.fill,
            allow_cross_rail: t.allow_cross_rail,
            buffer_bytes: t.buffer_bytes,
            buffer_infinite: t.buffer_infinite,
        })
    }

    fn build(inp: BuildInput) -> Result<Self, TopoError> {
        if inp.fill != 1.0 {
            return Err(TopoError::Illegal(format!(
                "fill must be 1.0, got {}",
                inp.fill
            )));
        }
        if inp.rails == 0 || inp.rails > u8::MAX as u32 {
            return Err(TopoError::Illegal("rails must be in 1..=255".into()));
        }
        if inp.g_tot % inp.rails != 0 {
            return Err(TopoError::Illegal(format!(
                "gpus {} not divisible by rails {}",
                inp.g_tot, inp.rails
            )));
        }
        if !OVERSUB_OK.contains(&inp.oversub) {
            return Err(TopoError::Illegal(format!(
                "oversub must be one of {:?}, got {}",
                OVERSUB_OK, inp.oversub
            )));
        }
        if inp.down == 0 || inp.leaf_radix == 0 {
            return Err(TopoError::Illegal("down and leaf_radix must be > 0".into()));
        }
        if inp.down % inp.oversub != 0 {
            return Err(TopoError::Illegal(format!(
                "down {} not divisible by oversub {}",
                inp.down, inp.oversub
            )));
        }

        let n = inp.g_tot / inp.rails;
        let r = inp.rails;
        let d = inp.down;
        let p = inp.leaf_radix;
        let k_omega = inp.oversub;
        let u = d / k_omega;
        let num_groups = n.div_ceil(d);
        let l = r * num_groups;
        let s = {
            let lu = (l as u64).saturating_mul(u as u64);
            lu.div_ceil(p as u64) as u32
        };

        let params = TopoParams {
            nodes: n,
            gpus_per_node: r,
            rails: r,
            leaf_radix: p,
            down: d,
            up: u,
            port_speed_gbps: inp.port_speed_gbps,
            scratch: inp.scratch,
            fill: inp.fill,
            allow_cross_rail: inp.allow_cross_rail,
            buffer_bytes: inp.buffer_bytes,
            buffer_infinite: inp.buffer_infinite,
        };

        let mut nodes = Vec::with_capacity(n as usize);
        let mut gpus = Vec::with_capacity(inp.g_tot as usize);
        for ni in 0..n {
            let mut node_gpus = Vec::with_capacity(r as usize);
            for ri in 0..r {
                let gid = GpuId(ni * r + ri);
                node_gpus.push(gid);
                gpus.push(Gpu {
                    id: gid,
                    node: NodeId(ni),
                    rail: RailId(ri as u8),
                    nic: NicId(gid.0),
                    avail: GpuAvail::Present,
                });
            }
            nodes.push(Node {
                id: NodeId(ni),
                gpus: node_gpus,
                present: true,
            });
        }

        let mut leaves = Vec::with_capacity(l as usize);
        for ri in 0..r {
            for g in 0..num_groups {
                leaves.push(Leaf {
                    id: LeafId(ri * num_groups + g),
                    rail: RailId(ri as u8),
                    group: g,
                    failed: false,
                });
            }
        }

        let spines: Vec<Spine> = (0..s)
            .map(|i| Spine {
                id: SpineId(i),
                failed: false,
            })
            .collect();

        let capacity_Bps = (inp.port_speed_gbps as u64) * 1_000_000_000 / 8;
        let scratch = inp.scratch;
        let mut links = Vec::new();
        let mut next = 0u32;
        let mut emit = |src: Endpoint, dst: Endpoint, links: &mut Vec<Link>| {
            links.push(Link {
                id: LinkId(next),
                src,
                dst,
                capacity_Bps,
                scratch,
                failed: false,
                bytes_this_epoch: 0,
            });
            next += 1;
        };

        // Host: NIC (n,r) ↔ leaf (r, floor(n/D)). LinkId 2*(n*R+r), +1.
        for ni in 0..n {
            for ri in 0..r {
                let nic = NicId(ni * r + ri);
                let leaf = LeafId(ri * num_groups + ni / d);
                emit(Endpoint::Nic(nic), Endpoint::Leaf(leaf), &mut links);
                emit(Endpoint::Leaf(leaf), Endpoint::Nic(nic), &mut links);
            }
        }

        // LS: §7.4. Neighbors in SpineId order; par copies Leaf→Spine + Spine→Leaf.
        let full_bipartite = s <= u && l <= p;
        for leaf in 0..l {
            let pars = if full_bipartite {
                (0..s)
                    .map(|spine| {
                        let par = u / s + if spine < (u % s) { 1 } else { 0 };
                        (spine, par)
                    })
                    .collect::<Vec<_>>()
            } else {
                let mut spines_hit: Vec<u32> = (0..u)
                    .map(|i| {
                        let raw = (leaf as u64) * (u as u64) + (i as u64);
                        (raw % (s as u64)) as u32
                    })
                    .collect();
                spines_hit.sort_unstable();
                spines_hit.into_iter().map(|spine| (spine, 1u32)).collect()
            };
            for (spine, par) in pars {
                if par == 0 {
                    continue;
                }
                for _ in 0..par {
                    emit(
                        Endpoint::Leaf(LeafId(leaf)),
                        Endpoint::Spine(SpineId(spine)),
                        &mut links,
                    );
                    emit(
                        Endpoint::Spine(SpineId(spine)),
                        Endpoint::Leaf(LeafId(leaf)),
                        &mut links,
                    );
                }
            }
        }

        Ok(Graph {
            epoch: EpochId(0),
            params,
            nodes,
            gpus,
            leaves,
            spines,
            links,
        })
    }

    /// Undirected host cables. Directed host links = 2×.
    pub fn e_host(&self) -> u32 {
        self.params.nodes * self.params.rails
    }

    /// Undirected LS cables. Directed LS links = 2×.
    pub fn e_ls(&self) -> u32 {
        (self.leaves.len() as u32) * self.params.up
    }

    pub fn b_bisect_gbps(&self) -> u64 {
        (self.leaves.len() as u64) * (self.params.up as u64) * (self.params.port_speed_gbps as u64)
            / 2
    }

    pub fn gpu(&self, id: GpuId) -> Option<&Gpu> {
        self.gpus.get(id.0 as usize).filter(|g| g.id == id)
    }

    pub fn leaf_of(&self, gpu: GpuId) -> Option<LeafId> {
        let g = self.gpu(gpu)?;
        Some(LeafId(
            g.rail.0 as u32 * self.num_groups() + g.node.0 / self.params.down,
        ))
    }

    pub fn num_groups(&self) -> u32 {
        self.params.nodes.div_ceil(self.params.down)
    }

    /// Nic → Leaf host uplink.
    pub fn host_up(&self, gpu: GpuId) -> Option<LinkId> {
        let nic = NicId(gpu.0);
        self.links
            .iter()
            .find(|l| l.src == Endpoint::Nic(nic) && matches!(l.dst, Endpoint::Leaf(_)))
            .map(|l| l.id)
    }

    /// Leaf → Nic host downlink.
    pub fn host_down(&self, gpu: GpuId) -> Option<LinkId> {
        let nic = NicId(gpu.0);
        self.links
            .iter()
            .find(|l| l.dst == Endpoint::Nic(nic) && matches!(l.src, Endpoint::Leaf(_)))
            .map(|l| l.id)
    }

    pub fn link(&self, id: LinkId) -> Option<&Link> {
        self.links.get(id.0 as usize).filter(|l| l.id == id)
    }

    /// Leaf→Spine cables for (leaf, spine), LinkId ascending.
    pub fn ups_to_spine(&self, leaf: LeafId, spine: SpineId) -> Vec<LinkId> {
        let mut v: Vec<LinkId> = self
            .links
            .iter()
            .filter(|l| l.src == Endpoint::Leaf(leaf) && l.dst == Endpoint::Spine(spine))
            .map(|l| l.id)
            .collect();
        v.sort_by_key(|id| id.0);
        v
    }

    /// Spine→Leaf cables, LinkId ascending.
    pub fn downs_from_spine(&self, spine: SpineId, leaf: LeafId) -> Vec<LinkId> {
        let mut v: Vec<LinkId> = self
            .links
            .iter()
            .filter(|l| l.src == Endpoint::Spine(spine) && l.dst == Endpoint::Leaf(leaf))
            .map(|l| l.id)
            .collect();
        v.sort_by_key(|id| id.0);
        v
    }

    pub fn common_spines(&self, sl: LeafId, dl: LeafId) -> Vec<SpineId> {
        let mut out = Vec::new();
        for spine in &self.spines {
            let sid = spine.id;
            if !self.ups_to_spine(sl, sid).is_empty() && !self.downs_from_spine(sid, dl).is_empty()
            {
                out.push(sid);
            }
        }
        out.sort_by_key(|s| s.0);
        out
    }
}

pub fn default_rails() -> u32 {
    DEFAULT_RAILS
}

pub fn format_endpoint(e: Endpoint) -> String {
    match e {
        Endpoint::Nic(n) => format!("Nic({})", n.0),
        Endpoint::Leaf(l) => format!("Leaf({})", l.0),
        Endpoint::Spine(s) => format!("Spine({})", s.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;

    fn n32() -> Graph {
        Graph::generate(256, 8, 1).expect("n32")
    }

    fn n64() -> Graph {
        Graph::generate(512, 8, 1).expect("n64")
    }

    fn host_leaf(g: &Graph, nic: NicId) -> LeafId {
        let link = g
            .links
            .iter()
            .find(|l| l.src == Endpoint::Nic(nic))
            .expect("host nic uplink");
        match link.dst {
            Endpoint::Leaf(id) => id,
            other => panic!("nic {} not wired to leaf: {:?}", nic.0, other),
        }
    }

    #[test]
    fn topo_n32_closed_form() {
        let g = n32();
        assert_eq!(g.params.nodes, 32);
        assert_eq!(g.leaves.len(), 8);
        assert_eq!(g.spines.len(), 4);
        assert_eq!(g.e_host(), 256);
        assert_eq!(g.e_ls(), 256);
        assert_eq!(g.b_bisect_gbps(), 51_200);
        assert_eq!(g.gpus.len(), 256);
        // Directed = 2× undirected.
        let host_dir = g
            .links
            .iter()
            .filter(|l| {
                matches!(
                    (l.src, l.dst),
                    (Endpoint::Nic(_), Endpoint::Leaf(_)) | (Endpoint::Leaf(_), Endpoint::Nic(_))
                )
            })
            .count();
        let ls_dir = g.links.len() - host_dir;
        assert_eq!(host_dir, 512);
        assert_eq!(ls_dir, 512);
        assert!(g.links.iter().all(|l| l.capacity_Bps == 50_000_000_000
            && l.scratch == 0.05
            && !l.failed
            && l.bytes_this_epoch == 0));

        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/topo/n32.toml");
        let toml = fs::read_to_string(&path).expect("n32.toml");
        let from_file = Graph::from_toml(&toml).expect("load n32.toml");
        assert_eq!(from_file.params.nodes, 32);
        assert_eq!(from_file.leaves.len(), 8);
        assert_eq!(from_file.spines.len(), 4);
        assert_eq!(from_file.e_host(), 256);
        assert_eq!(from_file.e_ls(), 256);
        assert_eq!(from_file.b_bisect_gbps(), 51_200);
        assert_eq!(from_file.links.len(), g.links.len());
    }

    #[test]
    fn topo_n64_closed_form() {
        let g = n64();
        assert_eq!(g.params.nodes, 64);
        assert_eq!(g.leaves.len(), 16);
        assert_eq!(g.spines.len(), 8);
        assert_eq!(g.e_host(), 512);
        assert_eq!(g.e_ls(), 512);
        assert_eq!(g.b_bisect_gbps(), 102_400);
        assert_eq!(g.gpus.len(), 512);
    }

    #[test]
    fn topo_rail_not_tor() {
        for g in [n32(), n64()] {
            for node in &g.nodes {
                let mut leaves = BTreeSet::new();
                for &gid in &node.gpus {
                    leaves.insert(host_leaf(&g, NicId(gid.0)));
                }
                assert_eq!(
                    leaves.len(),
                    node.gpus.len(),
                    "node {} ToR-mapped",
                    node.id.0
                );
            }
        }
    }

    #[test]
    fn topo_one_nic_per_gpu() {
        let g = n32();
        assert_eq!(g.gpus.len(), g.e_host() as usize);
        let nics: BTreeSet<u32> = g.gpus.iter().map(|gpu| gpu.nic.0).collect();
        let ids: BTreeSet<u32> = g.gpus.iter().map(|gpu| gpu.id.0).collect();
        assert_eq!(nics, ids);
        assert_eq!(nics.len(), g.gpus.len());
        for gpu in &g.gpus {
            assert_eq!(gpu.nic.0, gpu.id.0);
            assert_eq!(gpu.id.0, gpu.node.0 * g.params.rails + gpu.rail.0 as u32);
        }
    }

    #[test]
    fn topo_bisection_n32_leaf_not_spine() {
        let g = n32();
        // Same rail, two nodes in the single leaf group: Nic → Leaf → Nic.
        let a = NicId(0 * 8 + 0);
        let b = NicId(31 * 8 + 0);
        let leaf_a = host_leaf(&g, a);
        let leaf_b = host_leaf(&g, b);
        assert_eq!(leaf_a, leaf_b);
        for nic in [a, b] {
            for link in &g.links {
                let here = link.src == Endpoint::Nic(nic) || link.dst == Endpoint::Nic(nic);
                if here {
                    assert!(
                        !matches!(link.src, Endpoint::Spine(_))
                            && !matches!(link.dst, Endpoint::Spine(_)),
                        "same-rail host path used a spine: {:?}",
                        (link.src, link.dst)
                    );
                    assert!(matches!(
                        (link.src, link.dst),
                        (Endpoint::Nic(_), Endpoint::Leaf(_))
                            | (Endpoint::Leaf(_), Endpoint::Nic(_))
                    ));
                }
            }
        }
    }

    #[test]
    fn topo_ls_full_mesh_n32() {
        let g = n32();
        assert_eq!(g.leaves.len(), 8);
        assert_eq!(g.spines.len(), 4);
        for leaf in 0..8u32 {
            for spine in 0..4u32 {
                let cables = g
                    .links
                    .iter()
                    .filter(|l| {
                        l.src == Endpoint::Leaf(LeafId(leaf))
                            && l.dst == Endpoint::Spine(SpineId(spine))
                    })
                    .count();
                assert_eq!(cables, 8, "leaf {leaf} spine {spine}");
            }
        }
    }

    #[test]
    fn topo_rejects_fill_not_one() {
        let err = Graph::from_toml(
            r#"
kind = "rail_clos_2tier"
gpus = 256
rails = 8
oversub = 1
leaf_radix = 64
down = 32
port_speed_gbps = 400
scratch = 0.05
fill = 0.5
allow_cross_rail = true
buffer_bytes = 33554432
buffer_infinite = false
"#,
        )
        .unwrap_err();
        assert_eq!(err.e_code(), "E_TOPO");
    }

    #[test]
    fn topo_rejects_unknown_toml_key() {
        let err = Graph::from_toml(
            r#"
kind = "rail_clos_2tier"
gpus = 256
rails = 8
oversub = 1
leaf_radix = 64
down = 32
port_speed_gbps = 400
scratch = 0.05
fill = 1.0
allow_cross_rail = true
buffer_bytes = 33554432
buffer_infinite = false
nodes = 32
"#,
        )
        .unwrap_err();
        assert_eq!(err.e_code(), "E_SCHEMA");
    }
}
