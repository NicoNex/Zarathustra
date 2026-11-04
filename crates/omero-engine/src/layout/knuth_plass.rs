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
}

impl Default for Params {
    fn default() -> Self {
        Params { tolerance: 2.0, line_penalty: 10.0, double_hyphen: 3000.0, fitness_mismatch: 100.0 }
    }
}

/// One chosen break: the item index where the line ends and how its glue is set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Break {
    pub index: usize,
    /// Glue adjustment for the line ending here: >0 stretch, <0 shrink (in units of
    /// total stretch/shrink).
    pub ratio: f32,
}

#[derive(Clone, Copy)]
struct Node {
    index: usize,
    line: usize,
    fitness: u8,
    ratio: f32,
    /// Running sums at this break (after discarding glue/penalties that follow it).
    width: f32,
    stretch: f32,
    shrink: f32,
    demerits: f32,
    flagged: bool,
    prev: Option<usize>,
}

fn fitness(ratio: f32) -> u8 {
    match ratio {
        r if r < -0.5 => 0, // tight
        r if r <= 0.5 => 1, // decent
        r if r <= 1.0 => 2, // loose
        _ => 3,             // very loose
    }
}

/// Finds optimal breaks for lines of `widths(line_number)`; the last item must be a forced
/// break (`Penalty { cost: -INFINITY }`). Returns `None` when no feasible breaking exists at
/// this tolerance.
pub fn breaks(items: &[Item], widths: impl Fn(usize) -> f32, p: &Params) -> Option<Vec<Break>> {
    // Prefix sums of widths, stretch and shrink.
    let mut sum_w = Vec::with_capacity(items.len() + 1);
    let mut sum_y = Vec::with_capacity(items.len() + 1);
    let mut sum_z = Vec::with_capacity(items.len() + 1);
    let (mut w, mut y, mut z) = (0.0f32, 0.0f32, 0.0f32);
    for item in items {
        sum_w.push(w);
        sum_y.push(y);
        sum_z.push(z);
        match *item {
            Item::Box { width } => w += width,
            Item::Glue { width, stretch, shrink } => {
                w += width;
                y += stretch;
                z += shrink;
            }
            Item::Penalty { .. } => {}
        }
    }
    sum_w.push(w);
    sum_y.push(y);
    sum_z.push(z);

    let mut nodes = vec![Node {
        index: 0,
        line: 0,
        fitness: 1,
        ratio: 0.0,
        width: 0.0,
        stretch: 0.0,
        shrink: 0.0,
        demerits: 0.0,
        flagged: false,
        prev: None,
    }];
    let mut active: Vec<usize> = vec![0];

    for (b, item) in items.iter().enumerate() {
        let (pen_width, cost, flagged) = match *item {
            Item::Penalty { width, cost, flagged } if cost < INFINITY => (width, cost, flagged),
            // Glue is a legal break only right after a box.
            Item::Glue { .. } if b > 0 && matches!(items[b - 1], Item::Box { .. }) => (0.0, 0.0, false),
            _ => continue,
        };

        // Best candidate per fitness class for a break at `b`.
        let mut best: [Option<(f32, usize, f32)>; 4] = [None; 4];
        let mut i = 0;
        while i < active.len() {
            let ai = active[i];
            let a = nodes[ai];
            let line_w = sum_w[b] - a.width + pen_width;
            let target = widths(a.line);
            let ratio = if line_w < target {
                let stretch = sum_y[b] - a.stretch;
                if stretch > 0.0 {
                    (target - line_w) / stretch
                } else {
                    INFINITY
                }
            } else if line_w > target {
                let shrink = sum_z[b] - a.shrink;
                if shrink > 0.0 {
                    (target - line_w) / shrink
                } else {
                    -INFINITY
                }
            } else {
                0.0
            };

            // Overfull, or a forced break: this active node can't reach further.
            if ratio < -1.0 || cost <= -INFINITY {
                active.remove(i);
            } else {
                i += 1;
            }
            if (-1.0..=p.tolerance).contains(&ratio) {
                let badness = 100.0 * ratio.abs().powi(3);
                let base = (p.line_penalty + badness).powi(2);
                let mut d = if cost >= 0.0 {
                    base + cost * cost
                } else if cost > -INFINITY {
                    base - cost * cost
                } else {
                    base
                };
                if flagged && a.flagged {
                    d += p.double_hyphen;
                }
                let f = fitness(ratio);
                if f.abs_diff(a.fitness) > 1 {
                    d += p.fitness_mismatch;
                }
                let total = a.demerits + d;
                let slot = &mut best[f as usize];
                if slot.is_none_or(|(t, _, _)| total < t) {
                    *slot = Some((total, ai, ratio));
                }
            }
        }

        // Sums after the break: skip the glue and penalties that would start the next line.
        let (mut aw, mut ay, mut az) = (sum_w[b], sum_y[b], sum_z[b]);
        for (k, it) in items.iter().enumerate().skip(b) {
            match *it {
                Item::Box { .. } => break,
                Item::Penalty { cost, .. } if cost <= -INFINITY && k > b => break,
                _ => {
                    aw = sum_w[k + 1];
                    ay = sum_y[k + 1];
                    az = sum_z[k + 1];
                }
            }
        }
        for (f, slot) in best.iter().enumerate() {
            if let Some((total, prev, ratio)) = *slot {
                nodes.push(Node {
                    index: b,
                    line: nodes[prev].line + 1,
                    fitness: f as u8,
                    ratio,
                    width: aw,
                    stretch: ay,
                    shrink: az,
                    demerits: total,
                    flagged,
                    prev: Some(prev),
                });
                active.push(nodes.len() - 1);
            }
        }
        if active.is_empty() {
            return None;
        }
    }

    let last = items.len() - 1;
    let mut node = nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.index == last && n.prev.is_some())
        .min_by(|a, b| a.1.demerits.total_cmp(&b.1.demerits))?;
    let mut out = Vec::new();
    while let Some(prev) = node.1.prev {
        out.push(Break { index: node.1.index, ratio: node.1.ratio });
        node = (prev, &nodes[prev]);
    }
    out.reverse();
    Some(out)
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

    fn line_widths(items: &[Item], breaks: &[Break]) -> Vec<f32> {
        let mut out = Vec::new();
        let mut start = 0;
        for b in breaks {
            let w: f32 = items[start..b.index]
                .iter()
                .map(|i| match *i {
                    Item::Box { width } | Item::Glue { width, .. } => width,
                    Item::Penalty { .. } => 0.0,
                })
                .sum();
            out.push(w);
            start = b.index + 1;
        }
        out
    }

    #[test]
    fn every_line_fits_and_last_line_is_forced() {
        let items = words(&[3.0, 2.0, 4.0, 1.0, 5.0, 2.0, 3.0, 2.0, 4.0, 3.0]);
        let br = breaks(&items, |_| 10.0, &Params::default()).expect("feasible");
        assert_eq!(br.last().map(|b| b.index), Some(items.len() - 1));
        for (i, b) in br.iter().enumerate().take(br.len() - 1) {
            assert!((-1.0..=2.0).contains(&b.ratio), "line {i} ratio {}", b.ratio);
        }
        // Set width (natural + ratio·stretch/shrink) is exactly the measure on justified lines.
        let mut start = 0;
        for b in &br[..br.len() - 1] {
            let (mut w, mut y, mut z) = (0.0, 0.0, 0.0);
            for it in &items[start..b.index] {
                match *it {
                    Item::Box { width } => w += width,
                    Item::Glue { width, stretch, shrink } => (w, y, z) = (w + width, y + stretch, z + shrink),
                    Item::Penalty { .. } => {}
                }
            }
            let set = w + if b.ratio >= 0.0 { b.ratio * y } else { b.ratio * z };
            assert!((set - 10.0).abs() < 1e-3, "set width {set}");
            start = b.index + 1;
        }
    }

    #[test]
    fn prefers_even_lines_over_greedy() {
        // Greedy fills "aaa bb" (6) then leaves "c" alone; optimal balances lines.
        let items = words(&[3.0, 2.0, 1.0, 3.0, 2.0]);
        let br = breaks(&items, |_| 7.0, &Params::default()).expect("feasible");
        let widths = line_widths(&items, &br);
        let spread = widths.iter().take(widths.len() - 1).fold(0.0f32, |m, &w| m.max((7.0 - w).abs()));
        assert!(spread <= 2.0, "lines {widths:?}");
    }

    #[test]
    fn hyphen_penalty_is_used_only_when_it_pays() {
        // A long word with a hyphenation point lets the first line fill.
        let mut items = words(&[4.0, 4.0]);
        items.splice(
            2..3,
            [
                Item::Box { width: 3.0 },
                Item::Penalty { width: 1.0, cost: 50.0, flagged: true },
                Item::Box { width: 4.0 },
            ],
        );
        let br = breaks(&items, |_| 9.0, &Params::default()).expect("feasible");
        assert_eq!(br[0].index, 3, "breaks at the hyphen: {br:?}");
    }

    #[test]
    fn infeasible_at_tight_tolerance_is_none() {
        let items = words(&[9.0, 9.0]);
        assert!(breaks(&items, |_| 5.0, &Params { tolerance: 1.0, ..Params::default() }).is_none());
    }
}
