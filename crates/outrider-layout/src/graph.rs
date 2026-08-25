//! Layered graph layout for relationship views (inheritance, calls).
//! Nodes are simple boxes; edges point child -> parent and parents are
//! placed on higher (smaller-y) layers.
//!
//! Structure: split the graph into connected components, lay each one out
//! as a compact Sugiyama-lite cluster (longest-path layering + barycenter
//! ordering), then shelf-pack the clusters — largest first — into a
//! roughly square canvas. Isolated nodes are simply size-1 components, so
//! they fill in after the real hierarchies.

use std::collections::{BTreeMap, VecDeque};

use crate::pack::Rect;

/// Input node: an opaque index paired with a label length used for box width.
#[derive(Debug, Clone)]
pub struct GraphNode {
    /// Characters in the display label; box width scales with this.
    pub label_len: usize,
    /// Extra height beyond the base `node_h` (e.g. member rows), world px.
    pub extra_h: f64,
}

/// Sizing knobs for graph boxes, in world units.
#[derive(Debug, Clone, Copy)]
pub struct GraphConfig {
    pub node_h: f64,
    /// Width per label character.
    pub char_w: f64,
    pub min_w: f64,
    pub max_w: f64,
    /// Horizontal gap between siblings on a layer.
    pub h_gap: f64,
    /// Vertical gap between layers.
    pub v_gap: f64,
    /// Gap between packed clusters (both axes).
    pub cluster_gap: f64,
    /// Margin around the whole graph inside the root rect.
    pub margin: f64,
    /// Flow left-to-right (layers become columns, roots on the left)
    /// instead of top-to-bottom.
    pub left_to_right: bool,
}

impl Default for GraphConfig {
    fn default() -> Self {
        GraphConfig {
            node_h: 56.0,
            char_w: 9.0,
            min_w: 120.0,
            max_w: 440.0,
            h_gap: 48.0,
            v_gap: 96.0,
            cluster_gap: 120.0,
            margin: 64.0,
            left_to_right: false,
        }
    }
}

/// Output: one rect per input node (same indexing), plus the bounding
/// rect enclosing all nodes with the configured margin.
#[derive(Debug, Clone)]
pub struct GraphLayout {
    pub rects: Vec<Rect>,
    pub bounds: Rect,
}

/// A laid-out connected component, rects relative to its own (0,0) origin.
struct Cluster {
    members: Vec<usize>,
    rects: Vec<Rect>,
    w: f64,
    h: f64,
}

/// Lay out `nodes` with directed edges `(from, to)` meaning "from depends
/// on / inherits from to": `to` (the parent) is placed on a higher layer.
/// Cycles are tolerated (back edges are ignored for layering).
pub fn layout_graph(nodes: &[GraphNode], edges: &[(usize, usize)], cfg: &GraphConfig) -> GraphLayout {
    let n = nodes.len();
    if n == 0 {
        return GraphLayout {
            rects: Vec::new(),
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                w: cfg.margin * 2.0,
                h: cfg.margin * 2.0,
            },
        };
    }

    // Adjacency. Top-to-bottom: `to` is the parent (placed above) — the
    // inheritance convention, arrows pointing up at the base. Left-to-right:
    // `from` is the root (placed left) — the data-flow convention, arrows
    // pointing right along the pipeline.
    let mut parents: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(from, to) in edges {
        if from >= n || to >= n || from == to {
            continue;
        }
        let (child, parent) = if cfg.left_to_right { (to, from) } else { (from, to) };
        parents[child].push(parent);
        children[parent].push(child);
    }

    // Connected components (undirected), BFS.
    let mut comp_of: Vec<Option<usize>> = vec![None; n];
    let mut components: Vec<Vec<usize>> = Vec::new();
    for start in 0..n {
        if comp_of[start].is_some() {
            continue;
        }
        let id = components.len();
        let mut members = Vec::new();
        let mut q = VecDeque::new();
        comp_of[start] = Some(id);
        q.push_back(start);
        while let Some(i) = q.pop_front() {
            members.push(i);
            for &j in parents[i].iter().chain(children[i].iter()) {
                if comp_of[j].is_none() {
                    comp_of[j] = Some(id);
                    q.push_back(j);
                }
            }
        }
        members.sort();
        components.push(members);
    }

    // Lay out each component as a compact cluster.
    let mut clusters: Vec<Cluster> = components
        .into_iter()
        .map(|members| layout_component(nodes, &parents, &children, members, cfg))
        .collect();

    // Pack order: real hierarchies (multi-node) first, biggest first; then
    // isolated boxes, tallest first so shelves stay tidy.
    clusters.sort_by(|a, b| {
        let ka = (a.members.len() > 1, a.members.len(), a.h.to_bits(), a.w.to_bits());
        let kb = (b.members.len() > 1, b.members.len(), b.h.to_bits(), b.w.to_bits());
        kb.cmp(&ka)
    });

    // Target canvas width for a roughly square result: total area / sqrt.
    let total_area: f64 = clusters
        .iter()
        .map(|c| (c.w + cfg.cluster_gap) * (c.h + cfg.cluster_gap))
        .sum();
    let widest = clusters.iter().map(|c| c.w).fold(0.0, f64::max);
    let target_w = total_area.sqrt().max(widest);

    // Shelf pack.
    let mut rects: Vec<Rect> = vec![
        Rect {
            x: 0.0,
            y: 0.0,
            w: cfg.min_w,
            h: cfg.node_h,
        };
        n
    ];
    let mut shelf_x = 0.0;
    let mut shelf_y = cfg.margin;
    let mut shelf_h: f64 = 0.0;
    let mut max_x: f64 = 0.0;
    for c in &clusters {
        if shelf_x > 0.0 && shelf_x + c.w > target_w {
            shelf_y += shelf_h + cfg.cluster_gap;
            shelf_x = 0.0;
            shelf_h = 0.0;
        }
        for (k, &i) in c.members.iter().enumerate() {
            let r = c.rects[k];
            rects[i] = Rect {
                x: cfg.margin + shelf_x + r.x,
                y: shelf_y + r.y,
                w: r.w,
                h: r.h,
            };
        }
        shelf_x += c.w + cfg.cluster_gap;
        shelf_h = shelf_h.max(c.h);
        max_x = max_x.max(cfg.margin + shelf_x - cfg.cluster_gap);
    }
    let max_y = shelf_y + shelf_h;

    let bounds = Rect {
        x: 0.0,
        y: 0.0,
        w: max_x + cfg.margin,
        h: max_y + cfg.margin,
    };

    GraphLayout { rects, bounds }
}

/// Sugiyama-lite layout of one connected component. Returns rects relative
/// to the cluster's top-left (0,0).
fn layout_component(
    nodes: &[GraphNode],
    parents: &[Vec<usize>],
    children: &[Vec<usize>],
    members: Vec<usize>,
    cfg: &GraphConfig,
) -> Cluster {
    let node_w = |i: usize| -> f64 {
        (nodes[i].label_len as f64 * cfg.char_w).clamp(cfg.min_w, cfg.max_w)
    };
    let node_h = |i: usize| -> f64 { cfg.node_h + nodes[i].extra_h.max(0.0) };

    // Trivial cluster: one box.
    if members.len() == 1 {
        let i = members[0];
        let (w, h) = (node_w(i), node_h(i));
        return Cluster {
            members,
            rects: vec![Rect {
                x: 0.0,
                y: 0.0,
                w,
                h,
            }],
            w,
            h,
        };
    }

    // Longest-path layering from the roots (nodes with no parents).
    // layer 0 = topmost. Recursive DFS with a cycle guard.
    let mut layer: BTreeMap<usize, usize> = BTreeMap::new();
    let mut state: BTreeMap<usize, u8> = BTreeMap::new(); // 1 in-stack, 2 done
    fn depth_of(
        i: usize,
        parents: &[Vec<usize>],
        layer: &mut BTreeMap<usize, usize>,
        state: &mut BTreeMap<usize, u8>,
    ) -> usize {
        match state.get(&i) {
            Some(2) => return layer[&i],
            Some(1) => return 0, // cycle: treat as root
            _ => {}
        }
        state.insert(i, 1);
        let mut d = 0;
        for &p in &parents[i] {
            d = d.max(depth_of(p, parents, layer, state) + 1);
        }
        layer.insert(i, d);
        state.insert(i, 2);
        d
    }
    for &i in &members {
        depth_of(i, parents, &mut layer, &mut state);
    }

    let n_layers = layer.values().copied().max().unwrap_or(0) + 1;
    let mut rows: Vec<Vec<usize>> = vec![Vec::new(); n_layers];
    for &i in &members {
        rows[layer[&i]].push(i);
    }

    // Barycenter ordering sweeps.
    let mut pos: BTreeMap<usize, f64> = BTreeMap::new();
    for row in &rows {
        for (k, &i) in row.iter().enumerate() {
            pos.insert(i, k as f64);
        }
    }
    for sweep in 0..4 {
        let top_down = sweep % 2 == 0;
        let order: Vec<usize> = if top_down {
            (0..n_layers).collect()
        } else {
            (0..n_layers).rev().collect()
        };
        for li in order {
            let row = &mut rows[li];
            let mut keyed: Vec<(f64, usize)> = row
                .iter()
                .map(|&i| {
                    let neigh: &[usize] = if top_down { &parents[i] } else { &children[i] };
                    let bary = if neigh.is_empty() {
                        pos[&i]
                    } else {
                        neigh.iter().map(|&j| pos[&j]).sum::<f64>() / neigh.len() as f64
                    };
                    (bary, i)
                })
                .collect();
            keyed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            *row = keyed.into_iter().map(|(_, i)| i).collect();
            for (k, &i) in row.iter().enumerate() {
                pos.insert(i, k as f64);
            }
        }
    }

    let mut placed: BTreeMap<usize, Rect> = BTreeMap::new();
    let (cluster_w, cluster_h) = if cfg.left_to_right {
        // Layers become columns left to right (roots on the left); each
        // column's boxes stack vertically, centred on the tallest column.
        let col_h = |col: &[usize]| -> f64 {
            col.iter().map(|&i| node_h(i)).sum::<f64>()
                + cfg.h_gap * col.len().saturating_sub(1) as f64
        };
        let cluster_h = rows.iter().map(|c| col_h(c)).fold(0.0, f64::max);
        let mut x = 0.0;
        for col in &rows {
            if col.is_empty() {
                continue;
            }
            let ch = col_h(col);
            let cw = col.iter().map(|&i| node_w(i)).fold(0.0, f64::max);
            let mut y = (cluster_h - ch) / 2.0;
            for &i in col {
                let h = node_h(i);
                placed.insert(
                    i,
                    Rect {
                        x,
                        y,
                        w: node_w(i),
                        h,
                    },
                );
                y += h + cfg.h_gap;
            }
            x += cw + cfg.v_gap;
        }
        (x - cfg.v_gap, cluster_h)
    } else {
        // Rows stacked top to bottom, each row centred on the widest row.
        let row_w = |row: &[usize]| -> f64 {
            row.iter().map(|&i| node_w(i)).sum::<f64>()
                + cfg.h_gap * row.len().saturating_sub(1) as f64
        };
        let cluster_w = rows.iter().map(|r| row_w(r)).fold(0.0, f64::max);
        let mut y = 0.0;
        for row in &rows {
            if row.is_empty() {
                continue;
            }
            let rw = row_w(row);
            let rh = row.iter().map(|&i| node_h(i)).fold(0.0, f64::max);
            let mut x = (cluster_w - rw) / 2.0;
            for &i in row {
                let w = node_w(i);
                placed.insert(
                    i,
                    Rect {
                        x,
                        y,
                        w,
                        h: node_h(i),
                    },
                );
                x += w + cfg.h_gap;
            }
            y += rh + cfg.v_gap;
        }
        (cluster_w, y - cfg.v_gap)
    };

    let rects: Vec<Rect> = members.iter().map(|i| placed[i]).collect();
    Cluster {
        members,
        rects,
        w: cluster_w,
        h: cluster_h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nodes(lens: &[usize]) -> Vec<GraphNode> {
        lens.iter()
            .map(|&l| GraphNode {
                label_len: l,
                extra_h: 0.0,
            })
            .collect()
    }

    fn overlaps(a: &Rect, b: &Rect) -> bool {
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
    }

    #[test]
    fn parents_sit_above_children() {
        // 1 -> 0 and 2 -> 0: node 0 is the parent.
        let g = layout_graph(&nodes(&[5, 5, 5]), &[(1, 0), (2, 0)], &GraphConfig::default());
        assert!(g.rects[0].y < g.rects[1].y);
        assert!(g.rects[0].y < g.rects[2].y);
        assert!((g.rects[1].y - g.rects[2].y).abs() < 1e-9);
    }

    #[test]
    fn chain_layers_stack() {
        let g = layout_graph(&nodes(&[5, 5, 5]), &[(2, 1), (1, 0)], &GraphConfig::default());
        assert!(g.rects[0].y < g.rects[1].y);
        assert!(g.rects[1].y < g.rects[2].y);
    }

    #[test]
    fn cycle_does_not_hang() {
        let g = layout_graph(&nodes(&[5, 5]), &[(0, 1), (1, 0)], &GraphConfig::default());
        assert_eq!(g.rects.len(), 2);
    }

    #[test]
    fn isolated_nodes_pack_after_hierarchies() {
        // 1 -> 0 is a hierarchy; 2 is isolated. The hierarchy goes first
        // (top-left), the isolated box after it.
        let g = layout_graph(&nodes(&[5, 5, 5]), &[(1, 0)], &GraphConfig::default());
        let hier_top = g.rects[0].y.min(g.rects[1].y);
        assert!(g.rects[2].y >= hier_top);
        assert!(!overlaps(&g.rects[2], &g.rects[0]));
        assert!(!overlaps(&g.rects[2], &g.rects[1]));
    }

    #[test]
    fn separate_families_do_not_interleave() {
        // Family A: 1,2 -> 0. Family B: 4,5 -> 3. Each family's children
        // must sit directly under their own parent, not spread across a
        // shared global row.
        let g = layout_graph(
            &nodes(&[5, 5, 5, 5, 5, 5]),
            &[(1, 0), (2, 0), (4, 3), (5, 3)],
            &GraphConfig::default(),
        );
        let span = |ids: &[usize]| {
            let min = ids.iter().map(|&i| g.rects[i].x).fold(f64::INFINITY, f64::min);
            let max = ids
                .iter()
                .map(|&i| g.rects[i].x + g.rects[i].w)
                .fold(f64::NEG_INFINITY, f64::max);
            (min, max)
        };
        let (a0, a1) = span(&[0, 1, 2]);
        let (b0, b1) = span(&[3, 4, 5]);
        // Horizontal spans of the two families are disjoint OR they are on
        // different shelves (vertical spans disjoint).
        let ya = (g.rects[0].y, g.rects[1].y.max(g.rects[2].y) + g.rects[1].h);
        let yb = (g.rects[3].y, g.rects[4].y.max(g.rects[5].y) + g.rects[4].h);
        let x_disjoint = a1 <= b0 || b1 <= a0;
        let y_disjoint = ya.1 <= yb.0 || yb.1 <= ya.0;
        assert!(x_disjoint || y_disjoint, "families overlap: A x{a0}-{a1} B x{b0}-{b1}");
    }

    #[test]
    fn siblings_do_not_overlap() {
        let g = layout_graph(
            &nodes(&[10, 10, 10, 10]),
            &[(1, 0), (2, 0), (3, 0)],
            &GraphConfig::default(),
        );
        for i in 0..4 {
            for j in (i + 1)..4 {
                assert!(!overlaps(&g.rects[i], &g.rects[j]), "{i} overlaps {j}");
            }
        }
    }

    #[test]
    fn many_isolated_nodes_wrap_into_grid() {
        let g = layout_graph(&nodes(&vec![10; 60]), &[], &GraphConfig::default());
        let ys: std::collections::BTreeSet<i64> = g.rects.iter().map(|r| r.y as i64).collect();
        assert!(ys.len() > 1, "60 isolated nodes should span multiple rows");
        let aspect = g.bounds.w / g.bounds.h;
        assert!((0.2..=5.0).contains(&aspect), "grid aspect should be reasonable, got {aspect}");
    }

    #[test]
    fn no_rects_overlap_in_mixed_graph() {
        // 3 families of varying size plus isolated nodes with varying heights.
        let mut ns = nodes(&[8; 20]);
        ns[0].extra_h = 200.0;
        ns[15].extra_h = 120.0;
        let edges = [(1, 0), (2, 0), (3, 1), (5, 4), (7, 6), (8, 6), (9, 6), (10, 6)];
        let g = layout_graph(&ns, &edges, &GraphConfig::default());
        for i in 0..20 {
            for j in (i + 1)..20 {
                assert!(!overlaps(&g.rects[i], &g.rects[j]), "{i} overlaps {j}");
            }
        }
    }

    #[test]
    fn left_to_right_flows_from_source_to_sink() {
        // Data flow 0 -> 1 -> 2 and 1 -> 3: in LR mode the source (0) is
        // leftmost, sinks rightmost, and same-column boxes do not overlap.
        let cfg = GraphConfig {
            left_to_right: true,
            ..GraphConfig::default()
        };
        let g = layout_graph(&nodes(&[5, 5, 5, 5]), &[(0, 1), (1, 2), (1, 3)], &cfg);
        assert!(g.rects[0].x < g.rects[1].x);
        assert!(g.rects[1].x < g.rects[2].x);
        assert!((g.rects[2].x - g.rects[3].x).abs() < 1e-9);
        assert!(!overlaps(&g.rects[2], &g.rects[3]));
    }

    #[test]
    fn bounds_enclose_all_rects() {
        let g = layout_graph(&nodes(&[5, 8, 20]), &[(1, 0), (2, 1)], &GraphConfig::default());
        for r in &g.rects {
            assert!(r.x >= g.bounds.x);
            assert!(r.y >= g.bounds.y);
            assert!(r.x + r.w <= g.bounds.x + g.bounds.w);
            assert!(r.y + r.h <= g.bounds.y + g.bounds.h);
        }
    }

    #[test]
    fn empty_graph_is_fine() {
        let g = layout_graph(&[], &[], &GraphConfig::default());
        assert!(g.rects.is_empty());
        assert!(g.bounds.w > 0.0);
    }
}
