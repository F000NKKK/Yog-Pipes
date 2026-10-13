//! Destination ordering shared by every model (§6), so a policy works the
//! same in `buffered`, `routed`, `in_flight` and `flow`.

use crate::model::Rng;
use crate::registry::Policy;

/// Reorder `cands` (indices into a network's ports) for `policy`.
///
/// - `rotate` is the round-robin cursor (advanced by the caller).
/// - `prio(i)` is port `i`'s priority (endpoint + side).
/// - `dist(i)` is its hop distance from the source (only for `Nearest`).
///
/// `EvenSplit` orders like `RoundRobin`; the caller water-fills.
pub fn order(
    policy: Policy,
    cands: &mut [usize],
    rotate: usize,
    rng: &mut Rng,
    prio: impl Fn(usize) -> i32,
    dist: impl Fn(usize) -> u32,
) {
    let n = cands.len();
    if n <= 1 {
        return;
    }
    match policy {
        Policy::RoundRobin | Policy::EvenSplit => cands.rotate_left(rotate % n),
        Policy::Priority => {
            cands.rotate_left(rotate % n);
            // Stable: round-robin order survives within a priority.
            cands.sort_by_key(|&i| std::cmp::Reverse(prio(i)));
        }
        Policy::Nearest => {
            cands.rotate_left(rotate % n);
            cands.sort_by_key(|&i| dist(i));
        }
        Policy::Random => {
            for i in (1..n).rev() {
                let j = (rng.next() % (i as u64 + 1)) as usize;
                cands.swap(i, j);
            }
        }
    }
}

/// Split `amount` over sinks that can take `caps[i]` each so that every
/// sink gets an equal share of what is left, smaller takers first
/// (water-filling). Integer remainders go to the first sinks in order.
pub fn water_fill(amount: u64, caps: &[u64], out: &mut Vec<u64>) {
    out.clear();
    out.resize(caps.len(), 0);
    let mut idx: Vec<usize> = (0..caps.len()).filter(|&i| caps[i] > 0).collect();
    idx.sort_by_key(|&i| caps[i]);
    let mut left = amount;
    let mut k = idx.len() as u64;
    for &i in &idx {
        if left == 0 {
            break;
        }
        let share = left / k;
        let give = caps[i].min(share);
        out[i] = give;
        left -= give;
        k -= 1;
    }
    // Remainders (< number of sinks) to whoever still has room, in order.
    for i in 0..caps.len() {
        if left == 0 {
            break;
        }
        let more = (caps[i] - out[i]).min(left);
        out[i] += more;
        left -= more;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn water_fill_is_fair_and_exact() {
        let mut out = Vec::new();
        water_fill(100, &[10, 1000, 1000], &mut out);
        assert_eq!(out, vec![10, 45, 45]);
        water_fill(10, &[3, 3, 3], &mut out);
        assert_eq!(out.iter().sum::<u64>(), 9);
        water_fill(7, &[5, 5], &mut out);
        assert_eq!(out.iter().sum::<u64>(), 7);
    }

    #[test]
    fn priority_keeps_rotation_within_a_level() {
        let mut c = vec![0, 1, 2, 3];
        let mut rng = Rng(1);
        order(Policy::Priority, &mut c, 1, &mut rng, |i| (i == 3) as i32, |_| 0);
        assert_eq!(c, vec![3, 1, 2, 0]);
    }
}
