//! Max-min water-fill. Transcribed from docs/DESIGN.md §11.4.

use fabric_types::{LinkId, RejectCode};

use crate::paths::Path;

/// Max-min fill of `flows` on leftover indexed by `LinkId.0`. Empty `flows` → `Ok([])`.
pub fn water_fill(flows: &[Path], leftover: &[u64]) -> Result<Vec<u64>, RejectCode> {
    let n = flows.len();
    if n == 0 {
        return Ok(Vec::new());
    }

    let mut rem = leftover.to_vec();
    let max_idx = flows
        .iter()
        .flat_map(|p| p.links.iter().map(|e| e.0 as usize + 1))
        .max()
        .unwrap_or(0);
    if rem.len() < max_idx {
        rem.resize(max_idx, 0);
    }

    let mut rate = vec![0u64; n];
    let mut sat = vec![false; n];
    // Empty path: caller skips intra-node. Vacuous "all rem>=1" would crumb forever.
    for (i, p) in flows.iter().enumerate() {
        if p.links.is_empty() {
            sat[i] = true;
        }
    }

    loop {
        let active: Vec<usize> = (0..n).filter(|&f| !sat[f]).collect();
        if active.is_empty() {
            break;
        }
        let mut bottleneck = u64::MAX;
        for &f in &active {
            let mut min_share = u64::MAX;
            for &e in &flows[f].links {
                let c = count_active_on(&active, flows, e);
                if c == 0 {
                    continue;
                }
                min_share = min_share.min(rem_of(&rem, e) / c);
            }
            bottleneck = bottleneck.min(min_share);
        }
        if bottleneck == 0 || bottleneck == u64::MAX {
            break;
        }
        for &f in &active {
            rate[f] = rate[f].saturating_add(bottleneck);
        }
        for i in 0..rem.len() {
            let c = count_active_on(&active, flows, LinkId(i as u32));
            rem[i] = rem[i].saturating_sub(bottleneck.saturating_mul(c));
        }
        for &f in &active {
            if flows[f].links.iter().any(|&e| rem_of(&rem, e) == 0) {
                sat[f] = true;
            }
        }
    }

    loop {
        let mut progressed = false;
        for f in 0..n {
            if sat[f] {
                continue;
            }
            if flows[f].links.iter().all(|&e| rem_of(&rem, e) >= 1) {
                rate[f] = rate[f].saturating_add(1);
                for &e in &flows[f].links {
                    dec_rem(&mut rem, e);
                }
                progressed = true;
                if flows[f].links.iter().any(|&e| rem_of(&rem, e) == 0) {
                    sat[f] = true;
                }
            }
        }
        if !progressed {
            break;
        }
    }

    if rate.iter().all(|&r| r == 0) {
        return Err(RejectCode::ZeroLeftover);
    }
    if rate.iter().any(|&r| r == 0) {
        return Err(RejectCode::ResidualExhausted);
    }
    Ok(rate)
}

fn rem_of(rem: &[u64], e: LinkId) -> u64 {
    rem.get(e.0 as usize).copied().unwrap_or(0)
}

fn dec_rem(rem: &mut [u64], e: LinkId) {
    if let Some(slot) = rem.get_mut(e.0 as usize) {
        *slot = slot.saturating_sub(1);
    }
}

fn count_active_on(active: &[usize], flows: &[Path], e: LinkId) -> u64 {
    active
        .iter()
        .filter(|&&f| flows[f].links.contains(&e))
        .count() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_types::LinkId;

    #[test]
    fn joint_waterfill_maxmin() {
        let leftover = vec![100u64];
        let p = Path {
            links: vec![LinkId(0)],
        };
        let rates = water_fill(&[p.clone(), p], &leftover).expect("fill");
        assert_eq!(rates.len(), 2);
        let d = (rates[0] as i128 - rates[1] as i128).abs();
        assert!(d <= 1, "rates {rates:?} differ by {d}");
        assert_eq!(rates[0] + rates[1], 100);
    }
}
