//! Knuth–Plass optimal line breaking ("Breaking Paragraphs into Lines", 1981), as in TeX:
//! minimizes total demerits over the whole paragraph instead of filling lines greedily.

/// A paragraph item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Item {
    /// Unbreakable material of fixed width.
    Box { width: f32 },
    /// Breakable, stretchable space.
    Glue { width: f32, stretch: f32, shrink: f32 },
    /// Optional break; `width` is added only if the line breaks here (a hyphen).
    /// `flagged` marks hyphens: two flagged breaks in a row cost extra.
    Penalty { width: f32, cost: f32, flagged: bool },
}

impl Item {
    /// What this item contributes to a line it sits inside.
    fn metrics(&self) -> Sums {
        match *self {
            Item::Box { width } => Sums { w: width as f64, y: 0.0, z: 0.0 },
            Item::Glue { width, stretch, shrink } => Sums { w: width as f64, y: stretch as f64, z: shrink as f64 },
            Item::Penalty { .. } => Sums::default(),
        }
    }
}

/// Breaks with this cost are forbidden; `-INFINITY` forces a break.
pub const INFINITY: f32 = 10_000.0;

#[derive(Debug, Clone, Copy)]
pub struct Params {
    /// Maximum adjustment ratio a line may have before it counts as infeasible.
    pub tolerance: f32,
    /// Added to every line's demerits (TeX's \linepenalty).
    pub line_penalty: f32,
    /// Extra demerits for consecutive hyphenated lines.
    pub double_hyphen: f32,
    /// Extra demerits when adjacent lines differ by more than one fitness class.
    pub fitness_mismatch: f32,
    /// TeX's \emergencystretch: background stretch added to every line, for a last pass
    /// that must succeed (lines with a single word have no glue of their own).
    pub emergency_stretch: f32,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            tolerance: 2.0,
            line_penalty: 10.0,
            double_hyphen: 3000.0,
            fitness_mismatch: 100.0,
            emergency_stretch: 0.0,
        }
    }
}

/// One chosen break.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Break {
    /// Item index where the line ends (a glue or penalty, excluded from the line).
    pub index: usize,
    /// First item of the next line (discardable glue/penalties after the break skipped).
    pub next: usize,
    /// Glue adjustment for the line ending here: >0 stretch, <0 shrink (in units of the
    /// line's total stretch/shrink).
    pub ratio: f32,
}

/// Running totals; f64 so long paragraphs keep sub-pixel precision in differences.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Sums {
    w: f64,
    y: f64,
    z: f64,
}

impl std::ops::Add for Sums {
    type Output = Sums;
    fn add(self, o: Sums) -> Sums {
        Sums { w: self.w + o.w, y: self.y + o.y, z: self.z + o.z }
    }
}

impl std::ops::Sub for Sums {
    type Output = Sums;
    fn sub(self, o: Sums) -> Sums {
        Sums { w: self.w - o.w, y: self.y - o.y, z: self.z - o.z }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Fitness {
    Tight,
    Decent,
    Loose,
    VeryLoose,
}

impl Fitness {
    fn of(ratio: f64) -> Fitness {
        match ratio {
            r if r < -0.5 => Fitness::Tight,
            r if r <= 0.5 => Fitness::Decent,
            r if r <= 1.0 => Fitness::Loose,
            _ => Fitness::VeryLoose,
        }
    }
}

/// (line class, fitness) → (total demerits, previous node, ratio).
type Candidate = ((usize, Fitness), (f64, usize, f64));

#[derive(Clone, Copy)]
struct Node {
    /// Break item index (the start node uses 0).
    index: usize,
    /// First item of the line after this break.
    next: usize,
    /// Lines completed by this break.
    line: usize,
    fitness: Fitness,
    ratio: f64,
    /// Totals at `next`, where the following line starts.
    sums: Sums,
    demerits: f64,
    flagged: bool,
    prev: Option<usize>,
}

/// Finds optimal breaks. `widths[i]` is the measure of line `i`; the last entry applies to
/// all later lines. The last item must be a forced break (`Penalty { cost: -INFINITY }`).
/// Returns `None` when no breaking is feasible at `p.tolerance`; callers retry looser.
// ponytail: no final-hyphen demerits; callers retry looser (with emergency stretch) and fall
// back to greedy for pathological paragraphs.
pub fn breaks(items: &[Item], widths: &[f32], p: &Params) -> Option<Vec<Break>> {
    if items.is_empty() {
        return Some(Vec::new());
    }
    let measure = |line: usize| widths.get(line).or(widths.last()).copied().unwrap_or(0.0) as f64;
    // Line class: lines past the end of `widths` share a measure and can be merged.
    let class = |line: usize| line.min(widths.len().saturating_sub(1));

    let mut prefix = Vec::with_capacity(items.len() + 1);
    let mut acc = Sums::default();
    for item in items {
        prefix.push(acc);
        acc = acc + item.metrics();
    }
    prefix.push(acc);

    // Where a line starting after break `b` begins: skip glue and non-forced penalties.
    let mut next_start = vec![items.len(); items.len() + 1];
    for k in (0..items.len()).rev() {
        let discardable = match items[k] {
            Item::Box { .. } => false,
            Item::Penalty { cost, .. } => cost > -INFINITY,
            Item::Glue { .. } => true,
        };
        next_start[k] = if discardable { next_start[k + 1] } else { k };
    }

    let mut nodes = Vec::with_capacity(items.len());
    nodes.push(Node {
        index: 0,
        next: 0,
        line: 0,
        fitness: Fitness::Decent,
        ratio: 0.0,
        sums: Sums::default(),
        demerits: 0.0,
        flagged: false,
        prev: None,
    });
    let mut active: Vec<usize> = vec![0];
    // Best candidate per (line class, fitness) for the current breakpoint.
    let mut best: Vec<Candidate> = Vec::new();

    for (b, item) in items.iter().enumerate() {
        let (pen_width, cost, flagged) = match *item {
            Item::Penalty { width, cost, flagged } if cost < INFINITY => (width as f64, cost as f64, flagged),
            // Glue is a legal break only right after a box.
            Item::Glue { .. } if b > 0 && matches!(items[b - 1], Item::Box { .. }) => (0.0, 0.0, false),
            _ => continue,
        };
        let forced = cost <= -INFINITY as f64;

        best.clear();
        let mut kept = 0;
        for i in 0..active.len() {
            let ai = active[i];
            let a = nodes[ai];
            let line = prefix[b] - a.sums;
            let ratio = adjustment(line.w + pen_width, line.y + p.emergency_stretch as f64, line.z, measure(a.line));

            // Overfull lines and forced breaks end this active node's reach.
            if ratio >= -1.0 && !forced {
                active[kept] = ai;
                kept += 1;
            }
            if (-1.0..=p.tolerance as f64).contains(&ratio) {
                let fitness = Fitness::of(ratio);
                let total = a.demerits + demerits(ratio, cost, flagged && a.flagged, fitness, a.fitness, p);
                let key = (class(a.line + 1), fitness);
                match best.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, slot)) if total < slot.0 => *slot = (total, ai, ratio),
                    Some(_) => {}
                    None => best.push((key, (total, ai, ratio))),
                }
            }
        }
        active.truncate(kept);

        let next = next_start[b + 1].max(b + 1);
        for &((_, fitness), (demerits, prev, ratio)) in &best {
            nodes.push(Node {
                index: b,
                next,
                line: nodes[prev].line + 1,
                fitness,
                ratio,
                sums: prefix[next],
                demerits,
                flagged,
                prev: Some(prev),
            });
            active.push(nodes.len() - 1);
        }
        if active.is_empty() {
            return None;
        }
    }

    let last = items.len() - 1;
    let end = (1..nodes.len())
        .filter(|&n| nodes[n].index == last)
        .min_by(|&a, &b| nodes[a].demerits.total_cmp(&nodes[b].demerits))?;
    Some(backtrack(&nodes, end))
}

/// TeX's adjustment ratio for a line of natural width `w` with `y` stretch and `z` shrink.
fn adjustment(w: f64, y: f64, z: f64, target: f64) -> f64 {
    let inf = INFINITY as f64;
    if w < target {
        if y > 0.0 {
            (target - w) / y
        } else {
            inf
        }
    } else if w > target {
        if z > 0.0 {
            (target - w) / z
        } else {
            -inf
        }
    } else {
        0.0
    }
}

fn demerits(ratio: f64, cost: f64, double_hyphen: bool, f: Fitness, prev: Fitness, p: &Params) -> f64 {
    let badness = 100.0 * ratio.abs().powi(3);
    let base = (p.line_penalty as f64 + badness).powi(2);
    let inf = INFINITY as f64;
    let mut d = match cost {
        c if c >= 0.0 => base + c * c,
        c if c > -inf => base - c * c,
        _ => base,
    };
    if double_hyphen {
        d += p.double_hyphen as f64;
    }
    if (f as i8 - prev as i8).abs() > 1 {
        d += p.fitness_mismatch as f64;
    }
    d
}

fn backtrack(nodes: &[Node], end: usize) -> Vec<Break> {
    let mut out = Vec::new();
    let mut cur = end;
    while let Some(prev) = nodes[cur].prev {
        let n = nodes[cur];
        out.push(Break { index: n.index, next: n.next, ratio: n.ratio as f32 });
        cur = prev;
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Words of the given widths separated by 1-unit glue (stretch 1, shrink 0.5).
    fn words(ws: &[f32]) -> Vec<Item> {
        let mut items = Vec::new();
        for (i, &w) in ws.iter().enumerate() {
            if i > 0 {
                items.push(Item::Glue { width: 1.0, stretch: 1.0, shrink: 0.5 });
            }
            items.push(Item::Box { width: w });
        }
        items.push(Item::Glue { width: 0.0, stretch: INFINITY, shrink: 0.0 });
        items.push(Item::Penalty { width: 0.0, cost: -INFINITY, flagged: false });
        items
    }

    /// Set width of each non-final line: natural + ratio·stretch/shrink (+ hyphen width).
    fn set_widths(items: &[Item], breaks: &[Break]) -> Vec<f32> {
        let mut out = Vec::new();
        let mut start = 0;
        for b in &breaks[..breaks.len() - 1] {
            let s = items[start..b.index].iter().fold(Sums::default(), |acc, it| acc + it.metrics());
            let hyphen = match items[b.index] {
                Item::Penalty { width, .. } => width as f64,
                _ => 0.0,
            };
            let r = b.ratio as f64;
            out.push((s.w + hyphen + if r >= 0.0 { r * s.y } else { r * s.z }) as f32);
            start = b.next;
        }
        out
    }

    fn is_hyphen(items: &[Item], b: &Break) -> bool {
        matches!(items[b.index], Item::Penalty { flagged: true, .. })
    }

    fn with_hyphen(mut items: Vec<Item>, at: usize, left: f32, right: f32) -> Vec<Item> {
        items.splice(
            at..at + 1,
            [
                Item::Box { width: left },
                Item::Penalty { width: 1.0, cost: 50.0, flagged: true },
                Item::Box { width: right },
            ],
        );
        items
    }

    #[test]
    fn justified_lines_hit_the_measure_and_last_is_forced() {
        let items = words(&[3.0, 2.0, 4.0, 1.0, 5.0, 2.0, 3.0, 2.0, 4.0, 3.0]);
        let br = breaks(&items, &[10.0], &Params::default()).expect("feasible");
        assert_eq!(br.last().map(|b| b.index), Some(items.len() - 1));
        for w in set_widths(&items, &br) {
            assert!((w - 10.0).abs() < 1e-4, "set width {w}");
        }
    }

    /// Exhaustive search over every breaking: minimum total demerits, same scoring rules.
    fn brute_force(items: &[Item], measure: f64, p: &Params) -> Option<f64> {
        fn go(items: &[Item], start: usize, prev: Option<(Fitness, bool)>, measure: f64, p: &Params) -> Option<f64> {
            let mut best: Option<f64> = None;
            let mut sums = Sums::default();
            for b in start..items.len() {
                let (pen, cost, flagged) = match items[b] {
                    Item::Penalty { width, cost, flagged } if cost < INFINITY => (width as f64, cost as f64, flagged),
                    Item::Glue { .. } if b > start && matches!(items[b - 1], Item::Box { .. }) => (0.0, 0.0, false),
                    _ => {
                        sums = sums + items[b].metrics();
                        continue;
                    }
                };
                let ratio = adjustment(sums.w + pen, sums.y, sums.z, measure);
                if (-1.0..=p.tolerance as f64).contains(&ratio) {
                    let f = Fitness::of(ratio);
                    let (pf, pflag) = prev.unwrap_or((Fitness::Decent, false));
                    let d = demerits(ratio, cost, flagged && pflag, f, pf, p);
                    let rest = if b == items.len() - 1 {
                        Some(0.0)
                    } else {
                        let mut next = b + 1;
                        while next < items.len() && !matches!(items[next], Item::Box { .. }) {
                            next += 1;
                        }
                        go(items, next, Some((f, flagged)), measure, p)
                    };
                    if let Some(r) = rest {
                        best = Some(best.map_or(d + r, |x: f64| x.min(d + r)));
                    }
                }
                if cost <= -INFINITY as f64 || ratio < -1.0 {
                    break;
                }
                sums = sums + items[b].metrics();
            }
            best
        }
        go(items, 0, None, measure, p)
    }

    fn total_demerits(items: &[Item], br: &[Break], measure: f64, p: &Params) -> f64 {
        let (mut total, mut start, mut prev) = (0.0, 0, (Fitness::Decent, false));
        for b in br {
            let s = items[start..b.index].iter().fold(Sums::default(), |acc, it| acc + it.metrics());
            let (pen, cost, flagged) = match items[b.index] {
                Item::Penalty { width, cost, flagged } => (width as f64, cost as f64, flagged),
                _ => (0.0, 0.0, false),
            };
            let ratio = adjustment(s.w + pen, s.y, s.z, measure);
            let f = Fitness::of(ratio);
            total += demerits(ratio, cost, flagged && prev.1, f, prev.0, p);
            prev = (f, flagged);
            start = b.next;
        }
        total
    }

    #[test]
    fn matches_exhaustive_search() {
        // Deterministic pseudo-random paragraphs, some words with hyphenation points.
        let mut seed = 0x2545_f491u32;
        let mut rand = |n: u32| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed % n
        };
        let p = Params { tolerance: 3.0, ..Params::default() };
        for case in 0..200 {
            let n = 4 + rand(8) as usize;
            let mut items = words(&(0..n).map(|_| 1.0 + rand(5) as f32).collect::<Vec<_>>());
            if case % 2 == 0 {
                // Split a few words with a hyphen point.
                for k in (0..items.len()).rev() {
                    if let Item::Box { width } = items[k] {
                        if width >= 3.0 && rand(2) == 0 {
                            items = with_hyphen(items, k, 1.0, width - 1.0);
                        }
                    }
                }
            }
            let measure = 8.0 + rand(6) as f32;
            let kp = breaks(&items, &[measure], &p);
            let bf = brute_force(&items, measure as f64, &p);
            match (kp, bf) {
                (Some(br), Some(best)) => {
                    let got = total_demerits(&items, &br, measure as f64, &p);
                    assert!((got - best).abs() <= best.abs() * 1e-9 + 1e-6, "case {case}: {got} vs {best}");
                }
                (None, None) => {}
                (kp, bf) => panic!("case {case}: feasibility differs: kp {kp:?}, brute force {bf:?}"),
            }
        }
    }

    #[test]
    fn hyphenates_when_it_helps_and_not_otherwise() {
        let items = with_hyphen(words(&[4.0, 7.0]), 2, 3.0, 4.0);
        let br = breaks(&items, &[9.0], &Params::default()).expect("feasible");
        assert_eq!(br[0].index, 3, "fills the line at the hyphen: {br:?}");

        let items = with_hyphen(words(&[4.0, 4.0, 4.0]), 4, 2.0, 2.0);
        let br = breaks(&items, &[9.0], &Params::default()).expect("feasible");
        assert!(!br.iter().any(|b| is_hyphen(&items, b)), "a hyphen that doesn't pay is skipped: {br:?}");
    }

    #[test]
    fn varying_measures_are_not_merged() {
        // A short first line (like a drop cap), then wide lines.
        let items = words(&[2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0]);
        let br = breaks(&items, &[5.0, 11.0], &Params::default()).expect("feasible");
        let sets = set_widths(&items, &br);
        assert!((sets[0] - 5.0).abs() < 1e-4, "first line set to its own measure: {sets:?}");
        assert!(sets[1..].iter().all(|w| (w - 11.0).abs() < 1e-4), "{sets:?}");
    }

    #[test]
    fn empty_and_infeasible_inputs() {
        assert_eq!(breaks(&[], &[10.0], &Params::default()), Some(vec![]));
        let items = words(&[9.0, 9.0]);
        assert!(breaks(&items, &[5.0], &Params { tolerance: 1.0, ..Params::default() }).is_none());
    }
}
