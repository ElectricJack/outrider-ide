use std::collections::BTreeMap;

use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

use crate::pack::{
    exact_local_layout_cancellable, leaf_local_layout, ExactLayouts, LocalLayout, PackConfig,
    PackLayout, Rect,
};
use crate::zones::{build_profiles_cancellable, effective_role, RoleProfiles, SemanticRole};

#[derive(Debug, Clone, PartialEq)]
pub struct PackProgress {
    pub completed: usize,
    pub total: usize,
    pub snapshot: Option<PackLayout>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackCancelled;

pub fn pack_progressive<C, E>(
    tree: &SymbolTree,
    cfg: &PackConfig,
    max_snapshots: usize,
    is_cancelled: C,
    mut emit: E,
) -> Result<PackLayout, PackCancelled>
where
    C: Fn() -> bool,
    E: FnMut(PackProgress),
{
    if is_cancelled() {
        return Err(PackCancelled);
    }
    let draft = build_draft_layouts(&tree.root, cfg, &is_cancelled)?;
    let total = count_nodes(&tree.root);
    let draft_snapshot = absolute_from_layouts_cancellable(&tree.root, &draft, &is_cancelled)?;
    emit(PackProgress {
        completed: 0,
        total,
        snapshot: Some(draft_snapshot),
    });
    if is_cancelled() {
        return Err(PackCancelled);
    }

    let profiles = build_profiles_cancellable(&tree.root, &is_cancelled)?;
    let mut order = Vec::with_capacity(total);
    postorder_with_roles(
        &tree.root,
        SemanticRole::Source,
        &profiles,
        &mut order,
        &is_cancelled,
    )?;
    let milestones = snapshot_milestones(total, max_snapshots);
    let mut next_milestone = 1;
    let mut snapshot_pending = false;
    let mut exact = ExactLayouts::new();
    let mut final_layout = None;

    for (index, &(node, inherited_role)) in order.iter().enumerate() {
        if is_cancelled() {
            return Err(PackCancelled);
        }
        let mut child_sizes = BTreeMap::new();
        for child in &node.children {
            if is_cancelled() {
                return Err(PackCancelled);
            }
            child_sizes.insert(child.id.clone(), exact[&child.id].size);
        }
        let local = exact_local_layout_cancellable(
            node,
            inherited_role,
            &profiles,
            cfg,
            &child_sizes,
            &is_cancelled,
        )?;
        if is_cancelled() {
            return Err(PackCancelled);
        }
        exact.insert(node.id.clone(), local);

        let completed = index + 1;
        while next_milestone < milestones.len() && milestones[next_milestone] <= completed {
            snapshot_pending = true;
            next_milestone += 1;
        }
        let final_step = completed == total;
        let should_materialize = final_step || (snapshot_pending && !node.children.is_empty());
        let snapshot = if should_materialize {
            let layout = if final_step {
                absolute_from_layouts_cancellable(&tree.root, &exact, &is_cancelled)?
            } else {
                materialize_hybrid(&tree.root, &exact, cfg, &is_cancelled)?
            };
            snapshot_pending = false;
            if final_step {
                final_layout = Some(layout.clone());
            }
            Some(layout)
        } else {
            None
        };
        emit(PackProgress {
            completed,
            total,
            snapshot,
        });
    }

    Ok(final_layout.expect("a non-empty symbol tree always has a final layout"))
}

fn count_nodes(root: &SymbolNode) -> usize {
    1 + root.children.iter().map(count_nodes).sum::<usize>()
}

fn postorder_with_roles<'a, C>(
    node: &'a SymbolNode,
    inherited_role: SemanticRole,
    profiles: &RoleProfiles,
    out: &mut Vec<(&'a SymbolNode, SemanticRole)>,
    is_cancelled: &C,
) -> Result<(), PackCancelled>
where
    C: Fn() -> bool,
{
    if is_cancelled() {
        return Err(PackCancelled);
    }
    let folder = matches!(node.id.kind, SymbolKind::Folder);
    for child in &node.children {
        if is_cancelled() {
            return Err(PackCancelled);
        }
        let role = if folder {
            effective_role(&child.id, inherited_role, profiles)
        } else {
            SemanticRole::Source
        };
        postorder_with_roles(child, role, profiles, out, is_cancelled)?;
    }
    out.push((node, inherited_role));
    Ok(())
}

fn snapshot_milestones(total: usize, max_snapshots: usize) -> Vec<usize> {
    let snapshots = max_snapshots.max(2).min(total + 1);
    let denominator = (snapshots - 1) as u128;
    let mut milestones = Vec::with_capacity(snapshots);
    for k in 0..snapshots {
        let numerator = (k as u128) * (total as u128);
        let milestone = numerator.div_ceil(denominator) as usize;
        if milestones.last().copied() != Some(milestone) {
            milestones.push(milestone);
        }
    }
    milestones
}

#[allow(dead_code)] // Non-cancellable compatibility wrapper for exact draft behavior.
fn draft_local_layout(
    node: &SymbolNode,
    child_sizes: &BTreeMap<SymbolId, (f64, f64)>,
    cfg: &PackConfig,
) -> LocalLayout {
    draft_local_layout_cancellable(node, child_sizes, cfg, &|| false)
        .expect("never-cancel draft local layout")
}

fn draft_local_layout_cancellable<C>(
    node: &SymbolNode,
    child_sizes: &BTreeMap<SymbolId, (f64, f64)>,
    cfg: &PackConfig,
    is_cancelled: &C,
) -> Result<LocalLayout, PackCancelled>
where
    C: Fn() -> bool,
{
    if is_cancelled() {
        return Err(PackCancelled);
    }
    if node.children.is_empty() {
        return Ok(leaf_local_layout(node, cfg));
    }
    let mut children = BTreeMap::new();
    let mut y = 0.0_f64;
    let mut width = 0.0_f64;
    for child in &node.children {
        if is_cancelled() {
            return Err(PackCancelled);
        }
        let (child_width, child_height) = child_sizes[&child.id];
        children.insert(
            child.id.clone(),
            (cfg.gap, cfg.container_header + cfg.gap + y),
        );
        width = width.max(child_width);
        y += child_height + cfg.gap;
    }
    if is_cancelled() {
        return Err(PackCancelled);
    }
    Ok(LocalLayout {
        size: (width + 2.0 * cfg.gap, cfg.container_header + cfg.gap + y),
        children,
    })
}

fn build_draft_layouts(
    root: &SymbolNode,
    cfg: &PackConfig,
    is_cancelled: &impl Fn() -> bool,
) -> Result<ExactLayouts, PackCancelled> {
    fn build(
        node: &SymbolNode,
        cfg: &PackConfig,
        layouts: &mut ExactLayouts,
        is_cancelled: &impl Fn() -> bool,
    ) -> Result<(), PackCancelled> {
        if is_cancelled() {
            return Err(PackCancelled);
        }
        for child in &node.children {
            build(child, cfg, layouts, is_cancelled)?;
        }
        let mut child_sizes = BTreeMap::new();
        for child in &node.children {
            if is_cancelled() {
                return Err(PackCancelled);
            }
            child_sizes.insert(child.id.clone(), layouts[&child.id].size);
        }
        let local = draft_local_layout_cancellable(node, &child_sizes, cfg, is_cancelled)?;
        if is_cancelled() {
            return Err(PackCancelled);
        }
        layouts.insert(node.id.clone(), local);
        Ok(())
    }

    let mut layouts = ExactLayouts::new();
    build(root, cfg, &mut layouts, is_cancelled)?;
    Ok(layouts)
}

fn materialize_hybrid(
    root: &SymbolNode,
    exact: &ExactLayouts,
    cfg: &PackConfig,
    is_cancelled: &impl Fn() -> bool,
) -> Result<PackLayout, PackCancelled> {
    fn build(
        node: &SymbolNode,
        exact: &ExactLayouts,
        cfg: &PackConfig,
        layouts: &mut ExactLayouts,
        is_cancelled: &impl Fn() -> bool,
    ) -> Result<(), PackCancelled> {
        if is_cancelled() {
            return Err(PackCancelled);
        }
        for child in &node.children {
            build(child, exact, cfg, layouts, is_cancelled)?;
        }
        let local = if let Some(local) = exact.get(&node.id) {
            local.clone()
        } else {
            let mut child_sizes = BTreeMap::new();
            for child in &node.children {
                if is_cancelled() {
                    return Err(PackCancelled);
                }
                child_sizes.insert(child.id.clone(), layouts[&child.id].size);
            }
            draft_local_layout_cancellable(node, &child_sizes, cfg, is_cancelled)?
        };
        if is_cancelled() {
            return Err(PackCancelled);
        }
        layouts.insert(node.id.clone(), local);
        Ok(())
    }

    let mut layouts = ExactLayouts::new();
    build(root, exact, cfg, &mut layouts, is_cancelled)?;
    absolute_from_layouts_cancellable(root, &layouts, is_cancelled)
}

fn absolute_from_layouts_cancellable(
    root: &SymbolNode,
    layouts: &ExactLayouts,
    is_cancelled: &impl Fn() -> bool,
) -> Result<PackLayout, PackCancelled> {
    fn absolute(
        node: &SymbolNode,
        x: f64,
        y: f64,
        layouts: &ExactLayouts,
        rects: &mut BTreeMap<SymbolId, Rect>,
        is_cancelled: &impl Fn() -> bool,
    ) -> Result<(), PackCancelled> {
        if is_cancelled() {
            return Err(PackCancelled);
        }
        let local = &layouts[&node.id];
        rects.insert(
            node.id.clone(),
            Rect {
                x,
                y,
                w: local.size.0,
                h: local.size.1,
            },
        );
        for child in &node.children {
            let (relative_x, relative_y) = local.children[&child.id];
            absolute(
                child,
                x + relative_x,
                y + relative_y,
                layouts,
                rects,
                is_cancelled,
            )?;
        }
        Ok(())
    }

    let mut rects = BTreeMap::new();
    absolute(root, 0.0, 0.0, layouts, &mut rects, is_cancelled)?;
    Ok(PackLayout { rects })
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use outrider_index::{SymbolId, SymbolKind, SymbolNode, SymbolTree};

    use super::{build_draft_layouts, materialize_hybrid, snapshot_milestones};
    use crate::pack::ExactLayouts;
    use crate::{pack, pack_progressive, PackCancelled, PackConfig, PackLayout};

    const GEOMETRY_EPSILON: f64 = 1e-9;

    fn cfg() -> PackConfig {
        PackConfig {
            page_w: 120.0,
            line_step: 5.0,
            header: 10.0,
            container_header: 14.0,
            bottom_pad: 3.0,
            gap: 4.0,
            aspect: 1.6,
        }
    }

    fn node(kind: SymbolKind, path: &str, ordinal: u16, children: Vec<SymbolNode>) -> SymbolNode {
        SymbolNode {
            id: SymbolId {
                kind,
                qualified_path: path.into(),
                ordinal,
            },
            name: path
                .rsplit(['/', ':'])
                .find(|part| !part.is_empty())
                .unwrap_or(path)
                .into(),
            byte_range: None,
            signature: None,
            doc: None,
            measure: 1 + u64::from(ordinal),
            churn: 0.0,
            churn_count: 0,
            children,
        }
    }

    fn leaf(path: &str, ordinal: u16) -> SymbolNode {
        node(SymbolKind::File, path, ordinal, vec![])
    }

    fn tree(root: SymbolNode) -> SymbolTree {
        SymbolTree {
            root,
            repo_root: "root".into(),
        }
    }

    fn count_nodes(node: &SymbolNode) -> usize {
        1 + node.children.iter().map(count_nodes).sum::<usize>()
    }

    fn assert_complete_valid_geometry(node: &SymbolNode, layout: &PackLayout, cfg: &PackConfig) {
        let parent = layout
            .rects
            .get(&node.id)
            .expect("every node has a rectangle");
        assert!(parent.x.is_finite() && parent.y.is_finite());
        assert!(parent.w.is_finite() && parent.h.is_finite());
        assert!(parent.w > 0.0 && parent.h > 0.0);
        for child in &node.children {
            let rect = layout
                .rects
                .get(&child.id)
                .expect("every child has a rectangle");
            assert!(rect.x >= parent.x + cfg.gap);
            assert!(rect.y >= parent.y + cfg.container_header + cfg.gap);
            assert!(rect.x + rect.w <= parent.x + parent.w - cfg.gap + f64::EPSILON);
            assert!(rect.y + rect.h <= parent.y + parent.h - cfg.gap + f64::EPSILON);
            assert_complete_valid_geometry(child, layout, cfg);
        }
        for (index, left) in node.children.iter().enumerate() {
            let left_rect = layout.rects[&left.id];
            for right in node.children.iter().skip(index + 1) {
                let right_rect = layout.rects[&right.id];
                let separated = left_rect.x + left_rect.w + cfg.gap
                    <= right_rect.x + GEOMETRY_EPSILON
                    || right_rect.x + right_rect.w + cfg.gap <= left_rect.x + GEOMETRY_EPSILON
                    || left_rect.y + left_rect.h + cfg.gap <= right_rect.y + GEOMETRY_EPSILON
                    || right_rect.y + right_rect.h + cfg.gap <= left_rect.y + GEOMETRY_EPSILON;
                assert!(
                    separated,
                    "{} overlaps {} inside {}",
                    left.id.qualified_path, right.id.qualified_path, node.id.qualified_path
                );
            }
        }
    }

    fn varied_tree() -> SymbolTree {
        let cpp_items = vec![
            node(
                SymbolKind::Item { label: "fn".into() },
                "src/main.cpp::late",
                2,
                vec![],
            ),
            node(
                SymbolKind::Item {
                    label: "struct".into(),
                },
                "src/main.cpp::early",
                1,
                vec![],
            ),
        ];
        let markdown_items = vec![
            node(SymbolKind::Chunk, "docs/guide.md#second", 2, vec![]),
            node(SymbolKind::Chunk, "docs/guide.md#first", 1, vec![]),
        ];
        tree(node(
            SymbolKind::Folder,
            "root",
            0,
            vec![
                node(
                    SymbolKind::Folder,
                    "tests",
                    0,
                    vec![node(
                        SymbolKind::Folder,
                        "tests/unit",
                        0,
                        vec![leaf("tests/unit/plain.cpp", 0)],
                    )],
                ),
                node(SymbolKind::File, "src/main.cpp", 0, cpp_items),
                node(SymbolKind::File, "docs/guide.md", 0, markdown_items),
                leaf("assets/closest_hit.rchit", 0),
                leaf("examples/demo.rs", 0),
                leaf("vendor/generated.rs", 0),
            ],
        ))
    }

    fn assert_progressive_contract(tree: &SymbolTree, max_snapshots: usize) {
        let mut events = Vec::new();
        let progressive = pack_progressive(
            tree,
            &cfg(),
            max_snapshots,
            || false,
            |event| events.push(event),
        )
        .unwrap();

        let total = count_nodes(&tree.root);
        assert_eq!(
            events.iter().map(|e| e.completed).collect::<Vec<_>>(),
            (0..=total).collect::<Vec<_>>()
        );
        assert_eq!(events.first().unwrap().completed, 0);
        assert!(events.first().unwrap().snapshot.is_some());
        assert_eq!(events.last().unwrap().completed, total);
        assert_eq!(events.last().unwrap().snapshot.as_ref(), Some(&progressive));
        assert_eq!(progressive, pack(tree, &cfg()));
        let effective_cap = max_snapshots.max(2).min(total + 1);
        assert!(events.iter().filter(|e| e.snapshot.is_some()).count() <= effective_cap);
        for snapshot in events.iter().filter_map(|event| event.snapshot.as_ref()) {
            assert_complete_valid_geometry(&tree.root, snapshot, &cfg());
        }
    }

    #[test]
    fn draft_progress_milestones_and_final_match_for_all_caps() {
        let fixture = varied_tree();
        for cap in [0, 1, 2, 30, 1_000] {
            assert_progressive_contract(&fixture, cap);
        }
    }

    #[test]
    fn milestones_use_deduplicated_integer_ceiling_division() {
        assert_eq!(snapshot_milestones(10, 0), vec![0, 10]);
        assert_eq!(snapshot_milestones(10, 1), vec![0, 10]);
        assert_eq!(snapshot_milestones(10, 2), vec![0, 10]);
        assert_eq!(snapshot_milestones(10, 4), vec![0, 4, 7, 10]);
        assert_eq!(snapshot_milestones(3, 30), vec![0, 1, 2, 3]);
    }

    #[test]
    #[should_panic(expected = "overlaps")]
    fn snapshot_validator_rejects_siblings_without_configured_gap() {
        let fixture = tree(node(
            SymbolKind::Folder,
            "root",
            0,
            vec![leaf("root/left.rs", 0), leaf("root/right.rs", 1)],
        ));
        let mut layout = pack(&fixture, &cfg());
        let left = layout.rects[&fixture.root.children[0].id];
        layout
            .rects
            .insert(fixture.root.children[1].id.clone(), left);
        assert_complete_valid_geometry(&fixture.root, &layout, &cfg());
    }

    #[test]
    fn one_node_and_deep_trees_are_exact() {
        assert_progressive_contract(&tree(leaf("only.rs", 0)), 0);
        let mut deep = leaf("root/a/b/c/end.rs", 0);
        for (ordinal, path) in ["root/a/b/c", "root/a/b", "root/a", "root"]
            .into_iter()
            .enumerate()
        {
            deep = node(SymbolKind::Folder, path, ordinal as u16, vec![deep]);
        }
        assert_progressive_contract(&tree(deep), 30);
    }

    #[test]
    fn large_role_varied_folder_remains_exact() {
        let children = (0..180)
            .map(|index| {
                let name = match index % 6 {
                    0 => format!("root/source_{index}.rs"),
                    1 => format!("root/case_{index}_test.cpp"),
                    2 => format!("root/demo_{index}.rs"),
                    3 => format!("root/shader_{index}.frag"),
                    4 => format!("root/guide_{index}.md"),
                    _ => format!("root/generated_{index}.rs"),
                };
                leaf(&name, index)
            })
            .collect();
        assert_progressive_contract(&tree(node(SymbolKind::Folder, "root", 0, children)), 30);
    }

    #[test]
    fn cancellation_before_draft_emits_nothing() {
        let mut events = Vec::new();
        assert_eq!(
            pack_progressive(
                &varied_tree(),
                &cfg(),
                30,
                || true,
                |event| events.push(event)
            ),
            Err(PackCancelled)
        );
        assert!(events.is_empty());
    }

    #[test]
    fn cancellation_pulse_inside_initial_draft_child_loop_is_observed() {
        let fixture = tree(node(
            SymbolKind::Folder,
            "root",
            0,
            (0..512)
                .map(|i| leaf(&format!("root/file_{i}.rs"), i))
                .collect(),
        ));
        let calls = Cell::new(0usize);
        let result = build_draft_layouts(&fixture.root, &cfg(), &|| {
            calls.set(calls.get() + 1);
            calls.get() == 2_100
        });
        assert!(
            matches!(result, Err(PackCancelled)),
            "checks: {}",
            calls.get()
        );
        assert_eq!(calls.get(), 2_100);
    }

    #[test]
    fn initial_draft_inner_cancellation_emits_no_progress() {
        let fixture = tree(node(
            SymbolKind::Folder,
            "root",
            0,
            (0..512)
                .map(|i| leaf(&format!("root/file_{i}.rs"), i))
                .collect(),
        ));
        let calls = Cell::new(0usize);
        let mut events = Vec::new();
        let result = pack_progressive(
            &fixture,
            &cfg(),
            30,
            || {
                calls.set(calls.get() + 1);
                calls.get() == 2_300
            },
            |event| events.push(event),
        );
        assert_eq!(result, Err(PackCancelled));
        assert!(events.is_empty());
    }

    #[test]
    fn cancellation_pulse_inside_hybrid_draft_child_loop_is_observed() {
        let fixture = tree(node(
            SymbolKind::Folder,
            "root",
            0,
            (0..512)
                .map(|i| leaf(&format!("root/file_{i}.rs"), i))
                .collect(),
        ));
        let calls = Cell::new(0usize);
        let result = materialize_hybrid(&fixture.root, &ExactLayouts::new(), &cfg(), &|| {
            calls.set(calls.get() + 1);
            calls.get() == 2_100
        });
        assert!(
            matches!(result, Err(PackCancelled)),
            "checks: {}",
            calls.get()
        );
        assert_eq!(calls.get(), 2_100);
    }

    #[test]
    fn cancellation_after_selected_progress_emits_nothing_later() {
        let completed = Cell::new(0usize);
        let fixture = varied_tree();
        let mut events = Vec::new();
        let result = pack_progressive(
            &fixture,
            &cfg(),
            30,
            || completed.get() >= 4,
            |event| {
                completed.set(event.completed);
                events.push(event);
            },
        );
        assert_eq!(result, Err(PackCancelled));
        assert_eq!(
            events
                .iter()
                .map(|event| event.completed)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );
        for snapshot in events.iter().filter_map(|event| event.snapshot.as_ref()) {
            assert_complete_valid_geometry(&fixture.root, snapshot, &cfg());
        }
    }

    #[test]
    fn cancellation_interrupts_large_folder_refinement_without_invalid_snapshot() {
        let large_tree = tree(node(
            SymbolKind::Folder,
            "root",
            0,
            (0..512)
                .map(|i| leaf(&format!("root/file_{i}.rs"), i))
                .collect(),
        ));
        let completed = Cell::new(0usize);
        let root_work_checks = Cell::new(0usize);
        let mut snapshots = Vec::new();
        let result = pack_progressive(
            &large_tree,
            &cfg(),
            30,
            || {
                if completed.get() == 512 {
                    root_work_checks.set(root_work_checks.get() + 1);
                }
                root_work_checks.get() > 1_100
            },
            |event| {
                completed.set(event.completed);
                if let Some(snapshot) = event.snapshot {
                    snapshots.push(snapshot);
                }
            },
        );
        assert_eq!(result, Err(PackCancelled));
        assert_eq!(completed.get(), 512);
        assert_eq!(root_work_checks.get(), 1_101);
        assert!(!snapshots.is_empty());
        for snapshot in &snapshots {
            assert_complete_valid_geometry(&large_tree.root, snapshot, &cfg());
        }
    }
}
