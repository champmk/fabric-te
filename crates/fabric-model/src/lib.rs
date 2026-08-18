//! Collective closed forms and mix loader. Transcribed from docs/DESIGN.md §8 and §9.6.

#![forbid(unsafe_code)]

mod mix;

pub use fabric_types::{CollectiveKind, JobId, ProcessExit, SimTime};
pub use mix::{check_isolated, load_mix, JobSpec, Mix, MixError};

/// α = 10⁻⁶ s (§8.2).
const ALPHA_S: f64 = 1e-6;

/// Isolated check and omitted-deadline T use scratch-adjusted 47.5 GB/s (§9.6).
const ISOLATED_B_EFF_BPS: f64 = 47.5e9;

/// Mix-file / closed-form seconds → picoseconds. IEEE ties-to-even.
/// Non-finite input is rejected (same rule as `fabric_sim::s_to_ps`).
pub(crate) fn s_to_ps(x: f64) -> i128 {
    assert!(x.is_finite(), "s_to_ps: non-finite");
    (x * 1e12).round_ties_even() as i128
}

/// β = 1 / B_eff. B_eff in B/s. At 50e9 B/s this is 2e-11 s/B (20 ps/B), not 20 ns/B.
#[allow(non_snake_case)]
pub fn beta_s_per_byte(b_eff_Bps: f64) -> f64 {
    1.0 / b_eff_Bps
}

/// Integer µs: floor(ps / 1_000_000). Never /1000.
pub fn ps_to_us(ps: i128) -> i128 {
    ps / 1_000_000
}

/// Ring AllReduce. T = 2(p-1)α + 2(p-1)/p · βM. p<2 ⇒ 0.
#[allow(non_snake_case)]
pub fn ring_allreduce_ps(p: u32, payload_bytes: u64, b_eff_Bps: f64) -> i128 {
    if p < 2 {
        return 0;
    }
    let beta = beta_s_per_byte(b_eff_Bps);
    let m = payload_bytes as f64;
    let pf = f64::from(p);
    let pm1 = pf - 1.0;
    let t = 2.0 * pm1 * ALPHA_S + (2.0 * pm1 / pf) * beta * m;
    s_to_ps(t)
}

/// Pairwise AllToAll. T = (p-1)α + (p-1)/p · βM_tot. p<2 ⇒ 0.
#[allow(non_snake_case)]
pub fn pairwise_alltoall_ps(p: u32, payload_bytes: u64, b_eff_Bps: f64) -> i128 {
    if p < 2 {
        return 0;
    }
    let beta = beta_s_per_byte(b_eff_Bps);
    let m = payload_bytes as f64;
    let pf = f64::from(p);
    let pm1 = pf - 1.0;
    let t = pm1 * ALPHA_S + (pm1 / pf) * beta * m;
    s_to_ps(t)
}

/// Isolated T at 47.5e9 B/s, p = dp, full-fabric (no intra-node).
pub(crate) fn isolated_t_ps(collective: CollectiveKind, dp: u32, payload_bytes: u64) -> i128 {
    match collective {
        CollectiveKind::RingAllReduce => ring_allreduce_ps(dp, payload_bytes, ISOLATED_B_EFF_BPS),
        CollectiveKind::PairwiseAllToAll => {
            pairwise_alltoall_ps(dp, payload_bytes, ISOLATED_B_EFF_BPS)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const M_64_MIB: u64 = 67_108_864;
    const B_50: f64 = 50_000_000_000.0;
    const B_47_5: f64 = 47.5e9;

    /// Phase sum when every phase has the same B_eff: Σ_φ (α + chunk/B_eff).
    fn phase_sum_ps(p: u32, payload_bytes: u64, b_eff: f64, n_phases: u32) -> i128 {
        if p < 2 {
            return 0;
        }
        let chunk = (payload_bytes as f64) / f64::from(p);
        let d = ALPHA_S + chunk / b_eff;
        s_to_ps(f64::from(n_phases) * d)
    }

    #[test]
    fn model_ring_8x64mib() {
        assert_eq!(ring_allreduce_ps(8, M_64_MIB, B_50), 2_362_810_240);
    }

    #[test]
    fn model_ring_16x64mib() {
        assert_eq!(ring_allreduce_ps(16, M_64_MIB, B_50), 2_546_582_400);
    }

    #[test]
    fn model_a2a_8x64mib() {
        assert_eq!(pairwise_alltoall_ps(8, M_64_MIB, B_50), 1_181_405_120);
    }

    #[test]
    fn model_ring_8x64mib_47_5() {
        let t = ring_allreduce_ps(8, M_64_MIB, B_47_5);
        assert!(
            (t - 2_486_431_832).abs() <= 1,
            "got {t}, expected 2_486_431_832 ±1"
        );
    }

    #[test]
    fn model_beta_is_20ps_not_20ns() {
        assert_eq!(beta_s_per_byte(50e9), 2e-11);
        let m = M_64_MIB as f64;
        let bw_right = 1.75 * 2e-11 * m;
        let bw_wrong = 1.75 * 20e-9 * m;
        assert!((bw_wrong / bw_right - 1000.0).abs() < 1e-9);
    }

    #[test]
    fn model_us_is_ps_div_1e6() {
        assert_eq!(ps_to_us(1_000_000_000_000), 1_000_000);
        assert_ne!(1_000_000_000_000i128 / 1000, 1_000_000);
    }

    #[test]
    fn model_units_bytes_not_bits() {
        let golden = ring_allreduce_ps(8, M_64_MIB, B_50);
        let wrong = ring_allreduce_ps(8, M_64_MIB, 400e9);
        assert_ne!(wrong, golden);
        let alpha_ps = s_to_ps(14e-6);
        let bw_right = golden - alpha_ps;
        let bw_wrong = wrong - alpha_ps;
        let ratio = bw_right as f64 / bw_wrong as f64;
        assert!((ratio - 8.0).abs() < 1e-6, "ratio={ratio}");
    }

    #[test]
    fn model_phase_sum_eq_closed() {
        let closed_ring = ring_allreduce_ps(8, M_64_MIB, B_50);
        let sum_ring = phase_sum_ps(8, M_64_MIB, B_50, 2 * (8 - 1));
        assert!((sum_ring - closed_ring).abs() <= 1);

        let closed_a2a = pairwise_alltoall_ps(8, M_64_MIB, B_50);
        let sum_a2a = phase_sum_ps(8, M_64_MIB, B_50, 8 - 1);
        assert!((sum_a2a - closed_a2a).abs() <= 1);
    }

    #[test]
    fn model_p1_zero() {
        assert_eq!(ring_allreduce_ps(1, M_64_MIB, B_50), 0);
        assert_eq!(pairwise_alltoall_ps(1, M_64_MIB, B_50), 0);
        assert_eq!(ring_allreduce_ps(0, M_64_MIB, B_50), 0);
    }

    #[test]
    fn odd_ring_last_hop() {
        // p=3: 4α + (4/3)βM, not a p=2 formula.
        let beta = beta_s_per_byte(B_50);
        let expect = s_to_ps(4.0 * ALPHA_S + (4.0 / 3.0) * beta * (M_64_MIB as f64));
        assert_eq!(ring_allreduce_ps(3, M_64_MIB, B_50), expect);
        let p2 = ring_allreduce_ps(2, M_64_MIB, B_50);
        assert_ne!(ring_allreduce_ps(3, M_64_MIB, B_50), p2);
    }
}
