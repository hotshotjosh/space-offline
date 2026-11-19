// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0-plus-additions AND MIT
// Portions transcribed from carbonengine/destiny (src/Ballpark.cpp, src/Collision.cpp),
// Copyright (c) 2026 CCP Games, MIT License; see NOTICE.

//! Ship against ship: the client's own collision response, so that the
//! server's ball and the client's agree after a bump.
//!
//! **The client simulates the bump itself** from the flags and the
//! start-of-tick state it already has; the server sends nothing per contact
//! **[V]** (`bump-spec.md` §2.6). So this module is not a rule we chose. It is
//! `Ballpark::Potential` (`_destiny.dll` `0x18003edc0`, `Ballpark.cpp:2789`),
//! the swept-sphere time of impact (`0x18007d7e0`, `Collision.cpp:108`) and
//! the damping and "latest contact wins" bookkeeping around them, in the
//! server's units, over a state **sampled once at the start of the tick**.
//!
//! What the client does with a contact is not move a ball. It adds **one
//! tick of extra force** to the ball's own commanded force and integrates as
//! usual (`Evolve`, `0x18003ea93-0x18003eadd`) **[V]**. In this server's
//! terms a ball's commanded force is its commanded velocity `u = F * tau`
//! (`tau = M / K`), so a contact is a one-tick offset `du` to `u`, and the
//! module's whole output is that `du` per ball per tick.
//!
//! Every ball's response reads every other ball's start-of-tick state
//! (nothing is written during the pass), so planning is read-only and the
//! result is the same at any thread count: [`plan`].
//!
//! ## Carbon against the compiled DLL (bump lane, 2026-11-19)
//!
//! `Ballpark::Gradient` / `Potential` / `CollideTwoSpheres` at revision
//! `114e89fa` were diffed line by line against `bump-spec.md` §2 (the DLL
//! transcription). They agree on the time of impact, the contact branch, the
//! elastic exchange on raw `mass`, the overlap branch with its 1 m margin,
//! the recursion (depth < 2, the `M2/M1 > 25` rescale, the `1e-4` early
//! return), the push-out formula (including the `(1 + dt K / M1)` grouping,
//! which the spec had as `[I]`: Carbon's `-d1 / (dt * tmp * m1)` with
//! `tmp = 1 / (m1 + dt k)` is exactly it), the 0.85 damping and the
//! latest-contact-wins store. **Two differences, both in the pair filter:**
//!
//! 1. The spec's §2.2 names the byte at `+0xd3` `isInteractive` by flag order
//!    and writes the rule as "skip a candidate that is interactive when self
//!    is not". Carbon's `Ball` keeps `isFree, isGlobal, isMassive,
//!    isSpaceJunk` contiguously (`isInteractive` lives in the base class
//!    `Partitionable`) and the rule is `neighbor->isSpaceJunk &&
//!    !ball->isSpaceJunk` (`Ballpark.cpp:2772`, `:2681`). The DLL's test has
//!    the same shape, so the byte is `isSpaceJunk` **[I]**, and that is what
//!    [`Body::junk`] is. Falsifier: a `destiny-ref` dump of a non-junk ball
//!    overlapping a junk one that shows a push on both. Every ship the
//!    server sends has the junk bit (`0x1f`, `0x17`), so the rule never
//!    fires among ships either way.
//! 2. Carbon's `Quadratic` falls through to a division by zero when
//!    `a == 0 && b != 0`; the DLL (and the spec) return no contact. That case
//!    cannot occur (`a = |D|^2 == 0` forces `b = 2 D.d0 == 0`), so [`toi`]
//!    follows the DLL.
//!
//! Where Carbon reads `mNewPos`/`mNewVel` the DLL reads `+0x28/+0x178`: both
//! are the ball's state at the start of the tick, which is what [`Body`]
//! holds.

use crate::space_state::drag_axis;
use std::collections::HashMap;

/// The friction constant `K` (`park+0x48`): the time constant of a ball is
/// `tau = M / K`. `[I]` 1e6, falsified by a `tau` that mismatches the
/// client's align time (`space_state` derives `tau = agility * mass / 1e6`
/// and its align-time tests pin that against the client).
pub const K: f64 = 1.0e6;

/// A fixed ball's mass in the response (`0x1800ad970`, `0x18003ee60-0x18003ee95`) **[V]**.
pub const STATIC_MASS: f64 = 1.0e34;

/// A contact's push is stored at 85 % (`0x1800ad860`, `0x18003f94e`) **[V]**.
pub const DAMPING: f64 = 0.85;

/// Extra metres of separation asked for by an overlap (`0x1800ad8b8` region;
/// Carbon `collRadius - sqrt(p0q0_2) + 1`) **[V]**.
pub const OVERLAP_MARGIN: f64 = 1.0;

/// When the other ball is this many times heavier, the recursion's answer is
/// rescaled to the distance to get out (`0x1800ad8d8`) **[V]**.
pub const RATIO_RESCALE: f64 = 25.0;

/// Relative speed squared below which an overlapped pair is pushed apart
/// by force instead of left to the recursion (`0x1800ad7e0`) **[V]**.
pub const CLOSING_SPEED_SQ: f64 = 1.0e-4;

/// The overlap recursion stops here: `depth < 2` recurses (`0x18003f58f...`) **[V]**.
pub const MAX_DEPTH: u8 = 2;

/// `[A]` Uniform-grid cell edge, metres: 2 km (the first cut was 4 km; the
/// review halved it). Falsified by `bench_blob` (release) showing more than
/// 2 ms per serial tick for 1,000 ships in a 4 km cube, or more than 0.5 ms
/// for 1,000 in a 15 km cube. The numbers it printed are in the commit body.
pub const CELL: f64 = 2_000.0;

/// A body spanning more cells than this is kept in the grid's "big" list and
/// checked against everyone, rather than inserted into every cell.
const MAX_CELLS: i64 = 64;

/// `[A]` Bodies below which planning stays on the calling thread. Falsified
/// by `par::tests::spawn_cost` showing a plan of this size costs less than a
/// spawn.
pub const COLLIDE_PAR_MIN: usize = 512;

/// One ball's state at the start of a tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Body {
    pub id: i64,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    /// The commanded velocity (`F * tau`): what the ball is steered toward.
    pub u: [f64; 3],
    pub radius: f64,
    /// Raw mass (the elastic exchange uses it, not `agility * mass`).
    pub mass: f64,
    pub agility: f64,
    /// Whether the ball can move; a fixed ball answers nothing and weighs
    /// [`STATIC_MASS`].
    pub free: bool,
    /// `isSpaceJunk` (see the module doc, difference 1).
    pub junk: bool,
    pub cloaked: bool,
    pub missile: bool,
    /// A missile's owner and target ball ids (0: none).
    pub owner: i64,
    pub follow: i64,
}

impl Body {
    /// `M`: `agility * mass` for a free ball, [`STATIC_MASS`] for a fixed one.
    fn weight(&self) -> f64 {
        if self.free {
            self.agility * self.mass
        } else {
            STATIC_MASS
        }
    }

    /// Where this ball is, and how fast, `t` seconds on from the start of the tick.
    fn flown(&self, t: f64) -> ([f64; 3], [f64; 3]) {
        if !self.free {
            return (self.pos, [0.0; 3]);
        }
        let tau = self.weight() / K;
        let mut p = self.pos;
        let mut v = self.vel;
        for a in 0..3 {
            let (x, w) = drag_axis(self.pos[a], self.vel[a], self.u[a], tau, t);
            p[a] = x;
            v[a] = w;
        }
        (p, v)
    }
}

/// What one ball ends the tick holding: the client's `mLastC` and
/// `mLastCollision`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
    /// Time of the stored contact in `0..=1`; `-1` when none (`0x180035740`).
    pub last: f64,
    /// The stored one-tick offset to the commanded velocity.
    pub du: [f64; 3],
}

impl Default for Slot {
    fn default() -> Self {
        Slot {
            last: -1.0,
            du: [0.0; 3],
        }
    }
}

impl Slot {
    /// `0x18003f94e-0x18003fa14` **[V]**: damp by [`DAMPING`]; the **latest**
    /// contact in the tick wins, an overlap (`t = 0`) loses to any swept
    /// contact, and a tie keeps the stronger push.
    pub fn offer(&mut self, t: f64, du: [f64; 3]) {
        if t < self.last {
            return;
        }
        let du = scale(du, DAMPING);
        if t == self.last {
            if dot(du, du) > dot(self.du, self.du) {
                self.du = du;
            }
        } else {
            self.du = du;
            self.last = t;
        }
    }

    /// The offset to apply, when a contact was stored.
    pub fn contact(&self) -> Option<[f64; 3]> {
        (self.last >= 0.0).then_some(self.du)
    }
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: [f64; 3], k: f64) -> [f64; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn unit_of(a: [f64; 3]) -> Option<[f64; 3]> {
    let l = dot(a, a).sqrt();
    (l.is_finite() && l > 0.0).then(|| scale(a, 1.0 / l))
}

fn finite(a: [f64; 3]) -> bool {
    a.iter().all(|c| c.is_finite())
}

/// The time of impact of two spheres of summed radius `r` moving from
/// `p10`/`p20` to `p11`/`p21` over the tick, as a fraction of it
/// (`0x18007d7e0`, Carbon `CollideTwoSpheres`) **[V]**.
///
/// `0` when already touching; `-1` (the client's sentinel, kept so tests can
/// mirror it) when they do not touch within the tick.
pub fn toi(p10: [f64; 3], p11: [f64; 3], p20: [f64; 3], p21: [f64; 3], r: f64) -> f64 {
    let d0 = sub(p10, p20);
    let d0sq = dot(d0, d0);
    if d0sq <= r * r {
        return 0.0;
    }
    let d = sub(sub(p11, p10), sub(p21, p20));
    let a = dot(d, d);
    let b = 2.0 * dot(d, d0);
    if a == 0.0 {
        return -1.0;
    }
    let disc = b * b - 4.0 * a * (d0sq - r * r);
    if disc < 0.0 {
        return -1.0;
    }
    let t = (-b - disc.sqrt()) * 0.5 / a;
    if (0.0..=1.0).contains(&t) { t } else { -1.0 }
}

/// Whether `other` is a candidate for `me`'s response: the client's pair
/// filter (`0x18003ec45-0x18003ecad`, `0x180089d7c`; Carbon `Gradient` and
/// `GetNearbyBalls`).
pub fn candidate(me: &Body, other: &Body) -> bool {
    if other.id == me.id || other.cloaked {
        return false;
    }
    if me.missile {
        // A missile sees only the ball it follows (a defender missile, owner
        // < 0, sees everything but its owner), never its owner.
        if other.id == me.owner || (me.owner >= 0 && other.id != me.follow) {
            return false;
        }
    } else if other.missile {
        // A missile is no candidate of a ship, and in particular not of its
        // owner or its target.
        return false;
    }
    // `+0xd3`, `isSpaceJunk` [I]: a junk candidate is skipped by a non-junk ball.
    if other.junk && !me.junk {
        return false;
    }
    true
}

/// `Ballpark::Potential` for `me` against `other`, in the server's terms:
/// the contact (if any) is offered to `slot` as a one-tick offset `du` to
/// `me`'s commanded velocity. `dt` is the tick, `depth` the recursion depth.
pub fn respond(me: &Body, other: &Body, dt: f64, depth: u8, slot: &mut Slot) {
    if !(me.free && dt.is_finite() && dt > 0.0) {
        return;
    }
    let m1 = me.weight();
    let m2 = other.weight();
    let tau1 = m1 / K;
    if !(tau1.is_finite() && tau1 > 0.0) {
        return;
    }
    let r = me.radius + other.radius;
    // Where each would be at the end of the tick with no contact.
    let (p1, _) = me.flown(dt);
    let (q1, _) = other.flown(dt);
    let s = toi(me.pos, p1, other.pos, q1, r);
    if s == -1.0 {
        return;
    }
    let e1 = (-dt / tau1).exp();
    let denom = 1.0 - e1;
    if denom <= 0.0 {
        return;
    }
    let du;
    if s > 0.0 {
        // (b) The contact is inside the tick.
        let (pc, vc1) = me.flown(s * dt);
        let (qc, vc2) = other.flown(s * dt);
        let Some(n) = unit_of(sub(qc, pc)) else {
            return;
        };
        let s1 = dot(vc1, n);
        let s2 = dot(vc2, n);
        let bounced = if !other.free {
            // Someone hitting a fixed object: the radial speed is mirrored.
            sub(vc1, scale(n, 2.0 * s1))
        } else {
            // 1-D elastic exchange on the RAW masses (`0x18003f378-0x18003f3b2`).
            let (mm1, mm2) = (me.mass, other.mass);
            let tot = mm1 + mm2;
            if tot == 0.0 {
                return;
            }
            let s1p = ((mm1 - mm2) * s1 + 2.0 * mm2 * s2) / tot;
            add(vc1, scale(n, s1p - s1))
        };
        let rest = (1.0 - s) * dt;
        let vend = if rest > 0.0 {
            let mut v = bounced;
            for a in 0..3 {
                v[a] = drag_axis(pc[a], bounced[a], me.u[a], tau1, rest).1;
            }
            v
        } else {
            bounced
        };
        // The force that lands `vend`, as a commanded-velocity offset.
        du = sub(scale(sub(vend, scale(me.vel, e1)), 1.0 / denom), me.u);
    } else {
        // (c) Already overlapping at the start of the tick.
        let p0q0 = sub(other.pos, me.pos);
        let d2 = dot(p0q0, p0q0);
        let n = if d2 == 0.0 {
            // Right inside the other ball: an arbitrary but agreed normal.
            if me.id > other.id {
                [1.0, 0.0, 0.0]
            } else {
                [-1.0, 0.0, 0.0]
            }
        } else {
            match unit_of(p0q0) {
                Some(n) => n,
                None => return,
            }
        };
        let pen = r - d2.sqrt() + OVERLAP_MARGIN;
        let w1 = m2 / (m1 + m2) * pen;
        let w2 = m1 / (m1 + m2) * pen;
        // (At depth >= MAX_DEPTH the client logs "heading for infinite
        // recursion" and falls through to the forceful push.)
        if me.free && other.free && depth < MAX_DEPTH {
            // Move them apart along the normal, pretend the contact
            // happens there, and take that answer.
            let mut a = *me;
            let mut b = *other;
            a.pos = sub(me.pos, scale(n, w1));
            b.pos = add(other.pos, scale(n, w2));
            respond(&a, &b, dt, depth + 1, slot);
            if m2 / m1 > RATIO_RESCALE {
                // The lighter party's answer is rescaled to the distance
                // it has to get out by.
                if let Some(dir) = unit_of(slot.du) {
                    slot.du = scale(dir, pen * tau1);
                }
            }
            let rel = sub(me.vel, other.vel);
            if dot(rel, rel) > CLOSING_SPEED_SQ {
                return;
            }
        }
        // Stop, cancel the thrust along the normal, and leave by `w1` in one tick.
        let a1 = scale(
            sub(scale(n, -(w1 / dt) * (1.0 + dt * K / m1)), me.vel),
            1.0 / dt,
        );
        du = sub(scale(a1, tau1), scale(n, dot(me.u, n)));
    }
    if !finite(du) {
        return;
    }
    // A missile that hits its own target is not deflected.
    if me.missile && other.id == me.follow {
        return;
    }
    slot.offer(s, du);
}

/// A uniform hash grid over swept spheres: the broad phase.
///
/// Each body is a sphere of radius `reach` around its start-of-tick
/// position (`reach = radius + distance travelled this tick`), inserted into
/// every cell its box overlaps. Two bodies can touch in the tick only if
/// their centres are within `reach_a + reach_b` ([`Grid::candidates`] checks
/// exactly that), which is a superset of the client's partition pairs; the
/// narrow phase decides.
pub struct Grid {
    cell: f64,
    cells: HashMap<[i64; 3], Vec<u32>>,
    /// Bodies too large for the grid (a station-sized radius): everyone checks them.
    big: Vec<u32>,
}

fn cell_range(pos: [f64; 3], reach: f64, cell: f64) -> ([i64; 3], [i64; 3]) {
    let mut lo = [0i64; 3];
    let mut hi = [0i64; 3];
    for a in 0..3 {
        lo[a] = ((pos[a] - reach) / cell).floor() as i64;
        hi[a] = ((pos[a] + reach) / cell).floor() as i64;
    }
    (lo, hi)
}

fn span_of(lo: [i64; 3], hi: [i64; 3]) -> i64 {
    (0..3)
        .map(|a| hi[a] - lo[a] + 1)
        .try_fold(1i64, |acc, n| acc.checked_mul(n))
        .unwrap_or(i64::MAX)
}

impl Grid {
    /// Build the grid for `pos`/`reach` (index-aligned). Non-finite entries
    /// are left out.
    pub fn build(pos: &[[f64; 3]], reach: &[f64], cell: f64) -> Grid {
        let mut g = Grid {
            cell,
            cells: HashMap::with_capacity(pos.len()),
            big: Vec::new(),
        };
        for (i, (p, &r)) in pos.iter().zip(reach).enumerate() {
            if !finite(*p) || !r.is_finite() {
                continue;
            }
            let (lo, hi) = cell_range(*p, r, cell);
            if span_of(lo, hi) > MAX_CELLS {
                g.big.push(i as u32);
                continue;
            }
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        g.cells.entry([x, y, z]).or_default().push(i as u32);
                    }
                }
            }
        }
        g
    }

    /// Every index `j != i` whose swept sphere reaches `i`'s, ascending.
    pub fn candidates(&self, i: usize, pos: &[[f64; 3]], reach: &[f64], out: &mut Vec<u32>) {
        out.clear();
        let (p, r) = (pos[i], reach[i]);
        if !finite(p) || !r.is_finite() {
            return;
        }
        let (lo, hi) = cell_range(p, r, self.cell);
        if span_of(lo, hi) > MAX_CELLS {
            out.extend(0..pos.len() as u32);
        } else {
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        if let Some(v) = self.cells.get(&[x, y, z]) {
                            out.extend_from_slice(v);
                        }
                    }
                }
            }
            out.extend_from_slice(&self.big);
        }
        out.sort_unstable();
        out.dedup();
        out.retain(|&j| {
            let j = j as usize;
            j != i && finite(pos[j]) && reach[j].is_finite() && reaches(p, r, pos[j], reach[j])
        });
    }
}

/// The swept bound: whether two spheres of reach `ra` and `rb` around `a`
/// and `b` can touch.
fn reaches(a: [f64; 3], ra: f64, b: [f64; 3], rb: f64) -> bool {
    let d = sub(a, b);
    let r = ra + rb;
    dot(d, d) <= r * r
}

/// Brute force: the same pairs, found the slow way (the oracle).
pub fn candidates_brute(i: usize, pos: &[[f64; 3]], reach: &[f64]) -> Vec<u32> {
    (0..pos.len())
        .filter(|&j| j != i && reaches(pos[i], reach[i], pos[j], reach[j]))
        .map(|j| j as u32)
        .collect()
}

/// Whether any two of these swept spheres can touch: the gate that lets a
/// system with nothing near anything skip collision altogether.
///
/// A sweep over the x axis (sorted by the sphere's low edge, each sphere
/// compared with those whose low edge is under its high edge): two small
/// allocations and `O(n log n)`, no hash map, because the read path asks
/// this on every position read.
pub fn any_pair(pos: &[[f64; 3]], reach: &[f64]) -> bool {
    let mut order: Vec<u32> = (0..pos.len() as u32)
        .filter(|&i| finite(pos[i as usize]) && reach[i as usize].is_finite())
        .collect();
    if order.len() < 2 {
        return false;
    }
    let low = |i: u32| pos[i as usize][0] - reach[i as usize];
    order.sort_unstable_by(|&a, &b| low(a).total_cmp(&low(b)));
    for (k, &i) in order.iter().enumerate() {
        let high = pos[i as usize][0] + reach[i as usize];
        for &j in &order[k + 1..] {
            if low(j) > high {
                break;
            }
            if reaches(
                pos[i as usize],
                reach[i as usize],
                pos[j as usize],
                reach[j as usize],
            ) {
                return true;
            }
        }
    }
    false
}

/// The response of body `i` against the grid's candidates, sorted by ball id
/// (determinism).
fn plan_one(
    i: usize,
    bodies: &[Body],
    pos: &[[f64; 3]],
    reach: &[f64],
    grid: &Grid,
    dt: f64,
) -> Option<[f64; 3]> {
    let me = &bodies[i];
    if !me.free {
        return None;
    }
    let mut cand = Vec::new();
    grid.candidates(i, pos, reach, &mut cand);
    if cand.is_empty() {
        return None;
    }
    cand.sort_by_key(|&j| bodies[j as usize].id);
    let mut slot = Slot::default();
    for j in cand {
        let other = &bodies[j as usize];
        if candidate(me, other) {
            respond(me, other, dt, 0, &mut slot);
        }
    }
    slot.contact()
}

/// One tick's contacts: for each body, the offset to add to its commanded
/// velocity this tick (`None`: untouched). `bodies` are the massive balls,
/// sampled at the start of the tick; nothing here writes anything but the
/// returned vector, so the answer is the same at any `threads`.
pub fn plan(bodies: &[Body], dt: f64, threads: usize) -> Vec<Option<[f64; 3]>> {
    if bodies.len() < 2 {
        return vec![None; bodies.len()];
    }
    let pos: Vec<[f64; 3]> = bodies.iter().map(|b| b.pos).collect();
    let reach: Vec<f64> = bodies
        .iter()
        .map(|b| {
            let (end, _) = b.flown(dt);
            b.radius + dot(sub(end, b.pos), sub(end, b.pos)).sqrt()
        })
        .collect();
    let grid = Grid::build(&pos, &reach, CELL);
    let idx: Vec<usize> = (0..bodies.len()).collect();
    let one = |&i: &usize| plan_one(i, bodies, &pos, &reach, &grid, dt);
    match crate::par::par_map_with(threads, &idx, COLLIDE_PAR_MIN, one) {
        Ok(v) => v,
        Err(e) => {
            crate::errln!("[par] collision plan: {e}; serial");
            idx.iter().map(one).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ship(id: i64, pos: [f64; 3], vel: [f64; 3]) -> Body {
        Body {
            id,
            pos,
            vel,
            u: vel,
            radius: 50.0,
            mass: 1.0e6,
            agility: 1.0,
            free: true,
            junk: true,
            cloaked: false,
            missile: false,
            owner: 0,
            follow: 0,
        }
    }

    fn rock(id: i64, pos: [f64; 3], radius: f64) -> Body {
        Body {
            free: false,
            radius,
            mass: 0.0,
            u: [0.0; 3],
            ..ship(id, pos, [0.0; 3])
        }
    }

    fn near(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    fn vnear(a: [f64; 3], b: [f64; 3], tol: f64) -> bool {
        (0..3).all(|i| near(a[i], b[i], tol))
    }

    /// What a ball ends the tick with, given the offset: `u + du / 0.85`
    /// (the store damped it) held for one second.
    fn end_velocity(b: &Body, du: [f64; 3]) -> [f64; 3] {
        let tau = b.weight() / K;
        let mut v = [0.0; 3];
        for a in 0..3 {
            v[a] = drag_axis(b.pos[a], b.vel[a], b.u[a] + du[a] / DAMPING, tau, 1.0).1;
        }
        v
    }

    fn contact(me: &Body, other: &Body) -> Slot {
        let mut s = Slot::default();
        respond(me, other, 1.0, 0, &mut s);
        s
    }

    #[test]
    fn toi_is_zero_for_spheres_already_touching() {
        let p = [0.0; 3];
        assert_eq!(toi(p, p, [150.0, 0.0, 0.0], [150.0, 0.0, 0.0], 200.0), 0.0);
        assert_eq!(toi(p, p, [200.0, 0.0, 0.0], [200.0, 0.0, 0.0], 200.0), 0.0);
    }

    #[test]
    fn toi_is_minus_one_when_moving_apart() {
        let t = toi(
            [0.0; 3],
            [-500.0, 0.0, 0.0],
            [1000.0, 0.0, 0.0],
            [1500.0, 0.0, 0.0],
            200.0,
        );
        assert_eq!(t, -1.0);
        // And two balls with no relative motion that are apart.
        assert_eq!(
            toi(
                [0.0; 3],
                [5.0, 0.0, 0.0],
                [1000.0, 0.0, 0.0],
                [1005.0, 0.0, 0.0],
                200.0
            ),
            -1.0
        );
    }

    #[test]
    fn toi_of_a_head_on_pair_is_where_the_gap_closes() {
        // 1,000 m apart, closing 2,000 m in the tick, R = 200: the gap is
        // 800 m at 2,000 m per tick.
        let t = toi(
            [0.0; 3],
            [1000.0, 0.0, 0.0],
            [1000.0, 0.0, 0.0],
            [0.0, 0.0, 0.0],
            200.0,
        );
        assert!(near(t, 0.4, 1e-12), "{t}");
    }

    #[test]
    fn toi_ignores_contacts_after_the_tick() {
        let t = toi(
            [0.0; 3],
            [100.0, 0.0, 0.0],
            [1000.0, 0.0, 0.0],
            [1000.0, 0.0, 0.0],
            200.0,
        );
        assert_eq!(t, -1.0);
    }

    #[test]
    fn a_ball_bounces_off_a_static_by_mirroring_the_normal_speed() {
        // Flying at the rock along x with a slice of y: x mirrors, y stays.
        let me = ship(1, [0.0; 3], [300.0, 40.0, 0.0]);
        let rock = rock(2, [400.0, 0.0, 0.0], 100.0);
        let s = contact(&me, &rock);
        let du = s.contact().expect("a contact");
        // Contact at the first touch, 350 m short of the rock's centre line;
        // the normal is not exactly x, so recompute it the way the code does.
        let t = s.last;
        assert!(t > 0.0 && t < 1.0);
        let (pc, vc) = me.flown(t);
        let n = unit_of(sub(rock.pos, pc)).expect("a normal");
        let v_bounced = sub(vc, scale(n, 2.0 * dot(vc, n)));
        let vend_want = {
            let tau = me.weight() / K;
            let mut v = [0.0; 3];
            for a in 0..3 {
                v[a] = drag_axis(pc[a], v_bounced[a], me.u[a], tau, 1.0 - t).1;
            }
            v
        };
        assert!(vnear(end_velocity(&me, du), vend_want, 1e-9));
        // The tangential component is unchanged by the mirror.
        let tangent = |v: [f64; 3]| sub(v, scale(n, dot(v, n)));
        assert!(vnear(tangent(v_bounced), tangent(vc), 1e-9));
        assert!(near(dot(v_bounced, n), -dot(vc, n), 1e-9));
    }

    #[test]
    fn equal_masses_swap_normal_speeds() {
        let a = ship(1, [0.0; 3], [100.0, 0.0, 0.0]);
        let b = ship(2, [250.0, 0.0, 0.0], [-100.0, 0.0, 0.0]);
        let s = contact(&a, &b);
        let t = s.last;
        assert!(t > 0.0 && t < 1.0, "{t}");
        // Head on: the exchange reverses each, so each ends up flying away
        // along the line at what it was flying with (before the drag).
        let (_, vc) = a.flown(t);
        let du = s.contact().expect("contact");
        let vend = end_velocity(&a, du);
        let tau = a.weight() / K;
        // The bounced velocity at the contact is the other's: -vc.x. One
        // more (1-t) of drag toward u (= v0) then gives vend.
        let want = drag_axis(0.0, -vc[0], a.u[0], tau, 1.0 - t).1;
        assert!(near(vend[0], want, 1e-9), "{} {want}", vend[0]);
    }

    #[test]
    fn elastic_branch_uses_mass_not_agility_mass() {
        let mut a = ship(1, [0.0; 3], [100.0, 0.0, 0.0]);
        let mut b = ship(2, [250.0, 0.0, 0.0], [-100.0, 0.0, 0.0]);
        // Same raw masses, wildly different agility: the exchange at the
        // contact is the same (a symmetric swap), only the following of the
        // new velocity differs.
        a.agility = 0.5;
        b.agility = 4.0;
        let s = contact(&a, &b);
        let t = s.last;
        let (pc, vc) = a.flown(t);
        let (_, vc2) = b.flown(t);
        let _ = pc;
        // Equal raw masses: s1' = s2 exactly (the normal here is x).
        let n = [1.0, 0.0, 0.0];
        let s1p = ((a.mass - b.mass) * dot(vc, n) + 2.0 * b.mass * dot(vc2, n)) / (a.mass + b.mass);
        assert!(near(s1p, dot(vc2, n), 1e-9));
        // If agility * mass had been used the answer would differ.
        let wrong = ((a.weight() - b.weight()) * dot(vc, n) + 2.0 * b.weight() * dot(vc2, n))
            / (a.weight() + b.weight());
        assert!(!near(wrong, s1p, 1.0));
        let du = s.contact().expect("contact");
        let tau = a.weight() / K;
        let want = drag_axis(0.0, s1p, a.u[0], tau, 1.0 - t).1;
        // The tangential parts are zero on this line, so x carries it all.
        let vend = end_velocity(&a, du);
        assert!(near(vend[0], want, 1e-6), "{} {want}", vend[0]);
    }

    #[test]
    fn du_lands_the_bounced_velocity_at_the_end_of_the_tick() {
        let me = ship(1, [0.0; 3], [250.0, -30.0, 10.0]);
        let rock = rock(2, [350.0, 20.0, 0.0], 80.0);
        let s = contact(&me, &rock);
        let du = s.contact().expect("a contact");
        // Undo the damping: integrate with `u + du/0.85`, compare to the
        // velocity the bounce asked for.
        let t = s.last;
        let (pc, vc) = me.flown(t);
        let n = unit_of(sub(rock.pos, pc)).expect("normal");
        let bounced = sub(vc, scale(n, 2.0 * dot(vc, n)));
        let tau = me.weight() / K;
        for a in 0..3 {
            let want = drag_axis(pc[a], bounced[a], me.u[a], tau, 1.0 - t).1;
            let got = end_velocity(&me, du)[a];
            assert!(near(got, want, 1e-9), "axis {a}: {got} {want}");
        }
    }

    #[test]
    fn damping_is_eighty_five_percent() {
        let mut s = Slot::default();
        s.offer(0.5, [100.0, 0.0, 0.0]);
        assert!(near(s.du[0], 85.0, 1e-12));
        assert_eq!(DAMPING, 0.85);
    }

    #[test]
    fn overlap_pushes_apart_by_mass_share() {
        // Overlapping and at rest: a static rock pushes the ship out alone,
        // and two free ships split the depth by M2/(M1+M2).
        let a = ship(1, [0.0; 3], [0.0; 3]);
        let mut b = ship(2, [60.0, 0.0, 0.0], [0.0; 3]);
        b.mass = 3.0e6; // 3x heavier
        let r = a.radius + b.radius;
        let pen = r - 60.0 + OVERLAP_MARGIN;
        let (m1, m2) = (a.weight(), b.weight());
        let (w1, w2) = (m2 / (m1 + m2) * pen, m1 / (m1 + m2) * pen);
        assert!(near(w1 + w2, pen, 1e-12));
        assert!(near(w1 / w2, m2 / m1, 1e-12));
        // The lighter ship (a) gets the push, away from b (negative x),
        // and the heavier one's share is smaller.
        let sa = contact(&a, &b);
        let sb = contact(&b, &a);
        let da = sa.contact().expect("a pushed");
        let db = sb.contact().expect("b pushed");
        assert!(da[0] < 0.0 && db[0] > 0.0);
        assert!(da[0].abs() > db[0].abs(), "{da:?} {db:?}");
    }

    #[test]
    fn coincident_balls_split_on_x_by_id() {
        let a = ship(1, [10.0, 20.0, 30.0], [0.0; 3]);
        let b = ship(2, [10.0, 20.0, 30.0], [0.0; 3]);
        let da = contact(&a, &b).contact().expect("pushed");
        let db = contact(&b, &a).contact().expect("pushed");
        // Carbon's normal is +x for the higher id, -x for the lower, and the
        // push leaves along the opposite of it: the lower id goes +x.
        assert!(da[0] > 0.0 && db[0] < 0.0, "{da:?} {db:?}");
        assert!(near(da[1], 0.0, 1e-9) && near(da[2], 0.0, 1e-9));
    }

    #[test]
    fn recursion_stops_at_depth_two() {
        // At the depth limit the overlap is answered by the forceful push,
        // not by yet another recursion: the same call at depth MAX_DEPTH
        // and a ball closing on the other (which would return early at a
        // lower depth) still stores a push.
        let a = ship(1, [0.0; 3], [200.0, 0.0, 0.0]);
        let b = ship(2, [60.0, 0.0, 0.0], [-200.0, 0.0, 0.0]);
        let mut at_limit = Slot::default();
        respond(&a, &b, 1.0, MAX_DEPTH, &mut at_limit);
        assert!(at_limit.contact().is_some(), "the forceful push");
    }

    #[test]
    fn the_latest_contact_in_a_tick_wins_and_ties_keep_the_stronger() {
        let mut s = Slot::default();
        s.offer(0.0, [10.0, 0.0, 0.0]);
        s.offer(0.5, [1.0, 0.0, 0.0]);
        assert!(near(s.du[0], 0.85, 1e-12), "a later contact replaces");
        s.offer(0.2, [100.0, 0.0, 0.0]);
        assert!(near(s.du[0], 0.85, 1e-12), "an earlier one does not");
        s.offer(0.5, [0.5, 0.0, 0.0]);
        assert!(near(s.du[0], 0.85, 1e-12), "a weaker tie is ignored");
        s.offer(0.5, [4.0, 0.0, 0.0]);
        assert!(near(s.du[0], 3.4, 1e-12), "a stronger tie wins");
        assert_eq!(s.last, 0.5);
    }

    #[test]
    fn a_warping_ball_and_a_non_massive_ball_never_collide() {
        // A ball that is not massive is simply not given to `plan` (the
        // caller's rule); with only one body there is nothing to collide.
        let a = ship(1, [0.0; 3], [100.0, 0.0, 0.0]);
        assert_eq!(plan(&[a], 1.0, 1), vec![None]);
        // And two massive balls on a collision course do collide.
        let b = ship(2, [200.0, 0.0, 0.0], [-100.0, 0.0, 0.0]);
        let p = plan(&[a, b], 1.0, 1);
        assert!(p[0].is_some() && p[1].is_some());
        // A cloaked candidate is skipped, as the client's partition does.
        let mut cloaked = b;
        cloaked.cloaked = true;
        assert!(!candidate(&a, &cloaked));
    }

    #[test]
    fn a_missile_skips_its_owner() {
        let owner = ship(1, [0.0; 3], [0.0; 3]);
        let target = ship(2, [1000.0, 0.0, 0.0], [0.0; 3]);
        let other = ship(3, [500.0, 0.0, 0.0], [0.0; 3]);
        let mut m = ship(9, [10.0, 0.0, 0.0], [100.0, 0.0, 0.0]);
        m.missile = true;
        m.owner = 1;
        m.follow = 2;
        assert!(!candidate(&m, &owner), "never its owner");
        assert!(candidate(&m, &target), "its target");
        assert!(!candidate(&m, &other), "nothing else");
        assert!(
            !candidate(&owner, &m),
            "a ship is never a missile's candidate"
        );
        // A missile does not deflect off the ball it follows.
        let mut s = Slot::default();
        let mut close = target;
        close.pos = [100.0, 0.0, 0.0];
        respond(&m, &close, 1.0, 0, &mut s);
        assert!(s.contact().is_none());
    }

    #[test]
    fn a_junk_ball_is_skipped_by_a_non_junk_one() {
        // The filter's own rule (module doc, difference 1), pinned as ours.
        let mut clean = ship(1, [0.0; 3], [0.0; 3]);
        clean.junk = false;
        let junk = ship(2, [10.0, 0.0, 0.0], [0.0; 3]);
        assert!(!candidate(&clean, &junk));
        assert!(candidate(&junk, &clean));
        assert!(candidate(&junk, &junk.clone_with_id(3)));
    }

    impl Body {
        fn clone_with_id(&self, id: i64) -> Body {
            Body { id, ..*self }
        }
    }

    /// A tiny deterministic generator (no dependencies).
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 33
        }
        fn unit(&mut self) -> f64 {
            (self.next() % 1_000_000) as f64 / 1_000_000.0
        }
    }

    fn blob(n: usize, radius_m: f64, seed: u64) -> Vec<Body> {
        let mut rng = Lcg(seed);
        (0..n)
            .map(|i| {
                let mut p = [0.0; 3];
                for c in &mut p {
                    *c = (rng.unit() * 2.0 - 1.0) * radius_m;
                }
                let mut v = [0.0; 3];
                for c in &mut v {
                    *c = (rng.unit() * 2.0 - 1.0) * 300.0;
                }
                let mut b = ship(i as i64 + 1, p, v);
                b.radius = 20.0 + rng.unit() * 200.0;
                b
            })
            .collect()
    }

    #[test]
    fn planning_is_the_same_at_one_and_four_threads() {
        // 300 bodies in a small blob (plenty of contacts), and past
        // `COLLIDE_PAR_MIN` so four threads really split.
        for n in [300usize, COLLIDE_PAR_MIN + 100] {
            let bodies = blob(n, 3_000.0, 7);
            let one = plan(&bodies, 1.0, 1);
            let four = plan(&bodies, 1.0, 4);
            assert_eq!(one, four);
            assert!(one.iter().filter(|c| c.is_some()).count() > 10);
        }
    }

    #[test]
    fn the_broad_phase_finds_every_pair_brute_force_finds() {
        let mut bodies = blob(1_000, 40_000.0, 99);
        // Oversize bodies: a station-sized sphere and a titan, which must
        // be found from every side.
        bodies.push(rock(5_001, [0.0; 3], 20_000.0));
        bodies.push(rock(5_002, [30_000.0, 0.0, 0.0], 60_000.0));
        let pos: Vec<[f64; 3]> = bodies.iter().map(|b| b.pos).collect();
        let reach: Vec<f64> = bodies
            .iter()
            .map(|b| {
                let (end, _) = b.flown(1.0);
                b.radius + dot(sub(end, b.pos), sub(end, b.pos)).sqrt()
            })
            .collect();
        let grid = Grid::build(&pos, &reach, CELL);
        let mut out = Vec::new();
        let mut total = 0;
        for i in 0..bodies.len() {
            grid.candidates(i, &pos, &reach, &mut out);
            assert_eq!(out, candidates_brute(i, &pos, &reach), "body {i}");
            total += out.len();
        }
        assert!(total > 1_000, "the test must find something: {total}");
    }

    #[test]
    fn the_gate_sees_a_pair_only_when_one_could_touch() {
        let far = [[0.0; 3], [100_000.0, 0.0, 0.0]];
        assert!(!any_pair(&far, &[100.0, 100.0]));
        let near_ = [[0.0; 3], [150.0, 0.0, 0.0]];
        assert!(any_pair(&near_, &[100.0, 100.0]));
        assert!(!any_pair(&near_[..1], &[100.0]));
    }

    #[test]
    fn the_gate_agrees_with_brute_force() {
        for (n, size, seed) in [
            (200usize, 400_000.0, 3u64),
            (200, 20_000.0, 4),
            (50, 2_000.0, 5),
        ] {
            let bodies = blob(n, size, seed);
            let pos: Vec<[f64; 3]> = bodies.iter().map(|b| b.pos).collect();
            let reach: Vec<f64> = bodies.iter().map(|b| b.radius).collect();
            let brute = (0..n).any(|i| !candidates_brute(i, &pos, &reach).is_empty());
            assert_eq!(any_pair(&pos, &reach), brute, "{n} in {size}");
        }
    }

    /// Cost of one tick for a 1,000-ship blob; run with
    /// `cargo test --release -p eve-server collision::tests::bench_blob -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_blob() {
        for blob_m in [400_000.0, 15_000.0, 4_000.0] {
            let bodies = blob(1_000, blob_m, 5);
            for threads in [1usize, 4] {
                let t = std::time::Instant::now();
                let reps = 50;
                let mut contacts = 0;
                for _ in 0..reps {
                    contacts = plan(&bodies, 1.0, threads).iter().flatten().count();
                }
                let per = t.elapsed().as_secs_f64() * 1e3 / reps as f64;
                eprintln!(
                    "bench_blob: 1,000 bodies in a {blob_m} m cube, {threads} thread(s): {per:.3} ms per tick, {contacts} contacts"
                );
            }
            let pos: Vec<[f64; 3]> = bodies.iter().map(|b| b.pos).collect();
            let reach: Vec<f64> = bodies.iter().map(|b| b.radius).collect();
            let t = std::time::Instant::now();
            for _ in 0..50 {
                std::hint::black_box(any_pair(&pos, &reach));
            }
            eprintln!(
                "bench_blob: the gate, {:.3} ms",
                t.elapsed().as_secs_f64() * 1e3 / 50.0
            );
            // The cell occupancy the [A] cell size is judged by.
            let pos: Vec<[f64; 3]> = bodies.iter().map(|b| b.pos).collect();
            let reach = vec![200.0; pos.len()];
            let grid = Grid::build(&pos, &reach, CELL);
            let worst = grid.cells.values().map(Vec::len).max().unwrap_or(0);
            eprintln!(
                "bench_blob: occupied cells {}, most balls in one {worst}",
                grid.cells.len()
            );
        }
    }
}
