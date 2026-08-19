//! Spec validation.

use crate::spec::*;

/// A validation finding.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Violation {
    pub path: String,
    pub rule: &'static str,
    pub message: String,
    pub hard: bool,
}

/// Validate a ViewSpec. Returns a list of violations.
pub fn validate(
    spec: &ViewSpec,
    known_metrics: &dyn Fn(&str) -> bool,
    known_relations: &dyn Fn(&str) -> bool,
) -> Vec<Violation> {
    let mut violations = Vec::new();

    if spec.version != 1 {
        violations.push(Violation {
            path: "outriderView".to_string(),
            rule: "version",
            message: format!("unsupported version {}", spec.version),
            hard: true,
        });
    }

    // Validate set references
    for (i, layer) in spec.layers.iter().enumerate() {
        match layer {
            LayerSpec::Fill(fill) => {
                if !known_metrics(&fill.metric) {
                    violations.push(Violation {
                        path: format!("layers[{i}].fill.metric"),
                        rule: "unknown_metric",
                        message: format!("unknown metric '{}'", fill.metric),
                        hard: true,
                    });
                }
                if let Scale::Threshold(vals) = &fill.scale {
                    for w in vals.windows(2) {
                        if w[0] > w[1] {
                            violations.push(Violation {
                                path: format!("layers[{i}].fill.scale"),
                                rule: "threshold_order",
                                message: "threshold values must be ascending".to_string(),
                                hard: true,
                            });
                            break;
                        }
                    }
                }
            }
            LayerSpec::Marks(marks) => {
                let needs_basis = matches!(
                    marks.kind,
                    MarkKind::Hotspot
                        | MarkKind::Cycle
                        | MarkKind::LayeringViolation
                        | MarkKind::Custom
                );
                if needs_basis && marks.basis.is_none() {
                    violations.push(Violation {
                        path: format!("layers[{i}].marks.basis"),
                        rule: "structural_needs_basis",
                        message: format!("{:?} marks require a basis", marks.kind),
                        hard: true,
                    });
                }
            }
            LayerSpec::Edges(edges) => {
                match edges.source() {
                    Ok(EdgeSource::Relation(relation)) => {
                        if !known_relations(relation) {
                            violations.push(Violation {
                                path: format!("layers[{i}].edges.relation"),
                                rule: "unknown_relation",
                                message: format!("unknown relation '{relation}'"),
                                hard: true,
                            });
                        }
                    }
                    Ok(EdgeSource::Pairs(_)) => {}
                    Err(rule) => {
                        violations.push(Violation {
                            path: format!("layers[{i}].edges"),
                            rule,
                            message: "exactly one of relation or pairs must be set".to_string(),
                            hard: true,
                        });
                    }
                }

                if matches!(edges.direction, Some(Direction::Up) | Some(Direction::Down))
                    && edges.cross_boundary.is_none()
                {
                    violations.push(Violation {
                        path: format!("layers[{i}].edges.direction"),
                        rule: "edges.direction",
                        message: "direction needs a partition order".to_string(),
                        hard: true,
                    });
                }

                if edges.min_weight < 0.0 {
                    violations.push(Violation {
                        path: format!("layers[{i}].edges.minWeight"),
                        rule: "edges.min_weight",
                        message: "minWeight must be non-negative".to_string(),
                        hard: true,
                    });
                }
            }
            LayerSpec::Notes(NotesSpec(notes)) => {
                for (j, n) in notes.iter().enumerate() {
                    if n.source == NoteSource::Agent
                        && n.text.as_deref().map_or(true, |t| t.trim().is_empty())
                    {
                        violations.push(Violation {
                            path: format!("layers[{i}].notes[{j}].text"),
                            rule: "agent-note-needs-text",
                            message: "notes with source \"agent\" must carry text".into(),
                            hard: true,
                        });
                    }
                    if let Some((px, py)) = n.pin {
                        if !(0.0..=1.0).contains(&px) || !(0.0..=1.0).contains(&py) {
                            violations.push(Violation {
                                path: format!("layers[{i}].notes[{j}].pin"),
                                rule: "pin-out-of-range",
                                message: "pin is a relative offset in 0..=1".into(),
                                hard: true,
                            });
                        }
                    }
                    if let NoteAnchor::Range {
                        span: AnchorSpan::Lines([a, b]) | AnchorSpan::Range([a, b]),
                        ..
                    } = &n.at
                    {
                        if a > b {
                            violations.push(Violation {
                                path: format!("layers[{i}].notes[{j}].at"),
                                rule: "anchor-range-inverted",
                                message: "anchor range start must be <= end".into(),
                                hard: true,
                            });
                        }
                    }
                }
            }
            LayerSpec::Panel(panel) => {
                // EdgeGroups relation check
                if let PanelRows::EdgeGroups { relation, .. } = &panel.rows {
                    if !known_relations(relation) {
                        violations.push(Violation {
                            path: format!("layers[{i}].panel.rows.edgeGroups.relation"),
                            rule: "unknown_relation",
                            message: format!("unknown relation '{relation}'"),
                            hard: true,
                        });
                    }
                }
                // Matrix checks
                if let PanelRows::Matrix { space } = &panel.rows {
                    if space != "matrix" {
                        violations.push(Violation {
                            path: format!("layers[{i}].panel.rows.matrix.space"),
                            rule: "panel.matrix.space",
                            message: format!("matrix space must be \"matrix\", got \"{space}\""),
                            hard: true,
                        });
                    }
                    violations.push(Violation {
                        path: format!("layers[{i}].panel.rows.matrix"),
                        rule: "panel.matrix.unsupported",
                        message: "panel.matrix is not supported yet".to_string(),
                        hard: false,
                    });
                }
                // columns metric check
                for (j, col) in panel.columns.iter().enumerate() {
                    if !known_metrics(&col.0) {
                        violations.push(Violation {
                            path: format!("layers[{i}].panel.columns[{j}]"),
                            rule: "unknown_metric",
                            message: format!("unknown metric '{}'", col.0),
                            hard: true,
                        });
                    }
                }
                // sort_by metric check
                if let Some(SortKey::Metric(ref m)) = panel.sort_by {
                    if !known_metrics(&m.0) {
                        violations.push(Violation {
                            path: format!("layers[{i}].panel.sortBy"),
                            rule: "unknown_metric",
                            message: format!("unknown metric '{}'", m.0),
                            hard: true,
                        });
                    }
                }
                // limit zero
                if panel.limit == Some(0) {
                    violations.push(Violation {
                        path: format!("layers[{i}].panel.limit"),
                        rule: "panel.limit_zero",
                        message: "panel limit must be > 0".to_string(),
                        hard: true,
                    });
                }
            }
            LayerSpec::Mask(_) => {}
        }
    }

    // Check for duplicate panel ids
    let mut panel_ids: Vec<(usize, &str)> = Vec::new();
    for (i, layer) in spec.layers.iter().enumerate() {
        if let LayerSpec::Panel(panel) = layer {
            if let Some(ref id) = panel.id {
                if let Some(&(first_idx, _)) = panel_ids.iter().find(|(_, pid)| *pid == id.as_str()) {
                    violations.push(Violation {
                        path: format!("layers[{i}].panel.id"),
                        rule: "panel-id-duplicate",
                        message: format!("panel id \"{id}\" already used at layers[{first_idx}]"),
                        hard: true,
                    });
                }
                panel_ids.push((i, id));
            }
        }
    }

    // Validate set expressions (where/reach/community/layer)
    for (name, expr) in &spec.sets {
        validate_set_expr(
            expr,
            &format!("sets.{name}"),
            known_metrics,
            known_relations,
            &mut violations,
        );
    }

    // Check for unknown set references
    for (i, layer) in spec.layers.iter().enumerate() {
        check_set_refs_in_layer(layer, i, &spec.sets, &mut violations);
    }

    // Camera / tour validation
    if let Some(step_idx) = spec.camera.step {
        if step_idx >= spec.camera.steps.len() {
            violations.push(Violation {
                path: "camera.step".to_string(),
                rule: "camera.step_bounds",
                message: format!(
                    "camera.step ({step_idx}) >= steps.len ({})",
                    spec.camera.steps.len()
                ),
                hard: true,
            });
        }
    }
    for (i, step) in spec.camera.steps.iter().enumerate() {
        if let StepTarget::Frame(SetRef::Name(ref name)) = step.target {
            if !spec.sets.contains_key(name) {
                violations.push(Violation {
                    path: format!("camera.steps[{i}].frame"),
                    rule: "unknown_set",
                    message: format!("unknown set '{name}'"),
                    hard: true,
                });
            }
        }
        for (j, push_layer) in step.push.iter().enumerate() {
            check_set_refs_in_layer(
                push_layer,
                0, // layer_idx irrelevant for path
                &spec.sets,
                &mut violations,
            );
            // Fix the path for step-pushed layers
            if let Some(v) = violations.last() {
                if v.path.starts_with("layers[0]") {
                    let fixed_path = v.path.replace(
                        "layers[0]",
                        &format!("camera.steps[{i}].push[{j}]"),
                    );
                    let v = violations.last_mut().unwrap();
                    v.path = fixed_path;
                }
            }
        }
    }

    // Space validation
    if let Some(ref pack) = spec.space.pack {
        if let Some(gap) = pack.gap {
            if !(0.0..=64.0).contains(&gap) {
                violations.push(Violation {
                    path: "space.pack.gap".to_string(),
                    rule: "space.pack.gap",
                    message: "space.pack.gap must be within 0–64".to_string(),
                    hard: true,
                });
            }
        }
        if pack.max_display_lines == Some(0) {
            violations.push(Violation {
                path: "space.pack.maxDisplayLines".to_string(),
                rule: "space.pack.max_display_lines",
                message: "space.pack.maxDisplayLines must be >= 1".to_string(),
                hard: true,
            });
        }
    }
    match spec.space.kind {
        SpaceKind::Callgraph | SpaceKind::Matrix => {
            violations.push(Violation {
                path: "space.kind".to_string(),
                rule: "space.kind.unsupported",
                message: format!(
                    "space kind '{:?}' is not implemented; treemap is used",
                    spec.space.kind
                ),
                hard: false,
            });
        }
        SpaceKind::Treemap | SpaceKind::Graph => {}
    }

    violations
}

fn validate_set_expr(
    expr: &SetExpr,
    path: &str,
    known_metrics: &dyn Fn(&str) -> bool,
    known_relations: &dyn Fn(&str) -> bool,
    violations: &mut Vec<Violation>,
) {
    match expr {
        SetExpr::Where(w) => {
            if !known_metrics(&w.metric) {
                violations.push(Violation {
                    path: format!("{path}.where.metric"),
                    rule: "unknown_metric",
                    message: format!("unknown metric '{}'", w.metric),
                    hard: true,
                });
            }
            if let WhereValue::Percentile(ref s) = w.value {
                let valid = s.starts_with('p')
                    && s[1..].parse::<u32>().map_or(false, |n| n <= 100);
                if !valid {
                    violations.push(Violation {
                        path: format!("{path}.where.value"),
                        rule: "where.percentile",
                        message: format!("percentile must be p0..p100, got '{s}'"),
                        hard: true,
                    });
                }
            }
        }
        SetExpr::Reach(r) => {
            if !known_relations(&r.relation) {
                violations.push(Violation {
                    path: format!("{path}.reach.relation"),
                    rule: "unknown_relation",
                    message: format!("unknown relation '{}'", r.relation),
                    hard: true,
                });
            }
            validate_set_expr(&r.from, &format!("{path}.reach.from"), known_metrics, known_relations, violations);
        }
        SetExpr::Union(exprs) | SetExpr::Intersect(exprs) => {
            for (i, e) in exprs.iter().enumerate() {
                validate_set_expr(e, &format!("{path}[{i}]"), known_metrics, known_relations, violations);
            }
        }
        SetExpr::Diff(pair) => {
            validate_set_expr(&pair[0], &format!("{path}.diff[0]"), known_metrics, known_relations, violations);
            validate_set_expr(&pair[1], &format!("{path}.diff[1]"), known_metrics, known_relations, violations);
        }
        SetExpr::Not(inner) => {
            validate_set_expr(inner, &format!("{path}.not"), known_metrics, known_relations, violations);
        }
        SetExpr::Ancestors(r) | SetExpr::Descendants(r) | SetExpr::Children(r) | SetExpr::FileOf(r) => {
            if let SetRef::Inline(inner) = r.as_ref() {
                validate_set_expr(inner, path, known_metrics, known_relations, violations);
            }
        }
        _ => {}
    }
}

fn check_set_refs_in_layer(
    layer: &LayerSpec,
    layer_idx: usize,
    known_sets: &std::collections::BTreeMap<String, SetExpr>,
    violations: &mut Vec<Violation>,
) {
    let check_ref = |r: &SetRef, path: &str| -> Option<Violation> {
        if let SetRef::Name(name) = r {
            if !known_sets.contains_key(name) {
                // Inline expressions are always valid at this stage; only
                // named references can point at something that doesn't exist.
                return Some(Violation {
                    path: path.to_string(),
                    rule: "unknown_set",
                    message: format!("unknown set '{name}'"),
                    hard: true,
                });
            }
        }
        None
    };

    match layer {
        LayerSpec::Fill(fill) => {
            if let Some(ref d) = fill.domain {
                if let Some(v) = check_ref(d, &format!("layers[{layer_idx}].fill.domain")) {
                    violations.push(v);
                }
            }
        }
        LayerSpec::Marks(marks) => {
            let MarkTarget::Set(ref r) = marks.on;
            if let Some(v) = check_ref(r, &format!("layers[{layer_idx}].marks.on")) {
                violations.push(v);
            }
        }
        LayerSpec::Edges(edges) => {
            if let Some(ref r) = edges.within {
                if let Some(v) = check_ref(r, &format!("layers[{layer_idx}].edges.within")) {
                    violations.push(v);
                }
            }
            if let Some(ref r) = edges.incident_to {
                if let Some(v) = check_ref(r, &format!("layers[{layer_idx}].edges.incidentTo")) {
                    violations.push(v);
                }
            }
        }
        LayerSpec::Mask(mask) => {
            if let Some(ref r) = mask.dim_except {
                if let Some(v) = check_ref(r, &format!("layers[{layer_idx}].mask.dimExcept")) {
                    violations.push(v);
                }
            }
            if let Some(ref r) = mask.dim {
                if let Some(v) = check_ref(r, &format!("layers[{layer_idx}].mask.dim")) {
                    violations.push(v);
                }
            }
        }
        LayerSpec::Panel(panel) => {
            match &panel.rows {
                PanelRows::Set(r) => {
                    if let Some(v) = check_ref(r, &format!("layers[{layer_idx}].panel.rows.set")) {
                        violations.push(v);
                    }
                }
                PanelRows::EdgeGroups { of, .. } => {
                    if let Some(v) = check_ref(of, &format!("layers[{layer_idx}].panel.rows.edgeGroups.of")) {
                        violations.push(v);
                    }
                }
                PanelRows::Matrix { .. } => {}
            }
        }
        LayerSpec::Notes(_) => {}
    }
}
