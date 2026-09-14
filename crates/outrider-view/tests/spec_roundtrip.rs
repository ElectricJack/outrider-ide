use outrider_view::spec::*;

#[test]
fn view_spec_default_round_trips() {
    let spec = ViewSpec::default();
    let json = serde_json::to_string_pretty(&spec).unwrap();
    let back: ViewSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(spec, back);
}

#[test]
fn fill_layer_round_trips() {
    let spec = LayerSpec::Fill(FillSpec {
        metric: "churn".to_string(),
        channel: FillChannel::Stripe,
        scale: Scale::Percentile,
        domain: None,
        ramp: None,
    });
    let json = serde_json::to_string(&spec).unwrap();
    let back: LayerSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(spec, back);
}

#[test]
fn marks_layer_round_trips() {
    let spec = LayerSpec::Marks(MarksSpec {
        on: MarkTarget::Set(SetRef::Name("focusSet".to_string())),
        kind: MarkKind::FocusRing,
        label: None,
        basis: None,
    });
    let json = serde_json::to_string(&spec).unwrap();
    let back: LayerSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(spec, back);
}

#[test]
fn deny_unknown_fields_rejects_typo() {
    let json = r#"{"outriderView":1,"typo":"x"}"#;
    let result = serde_json::from_str::<ViewSpec>(json);
    assert!(result.is_err());
}

#[test]
fn set_expr_ids_round_trips() {
    let expr = SetExpr::Ids(vec!["$focus".to_string(), "fn:src/a.rs::foo".to_string()]);
    let json = serde_json::to_string(&expr).unwrap();
    let back: SetExpr = serde_json::from_str(&json).unwrap();
    assert_eq!(expr, back);
}

#[test]
fn set_expr_neighbors_round_trips() {
    let expr = SetExpr::Neighbors("focus".to_string());
    let json = serde_json::to_string(&expr).unwrap();
    let back: SetExpr = serde_json::from_str(&json).unwrap();
    assert_eq!(expr, back);
}

#[test]
fn panel_set_rows_round_trips() {
    let json = r#"{"panel": {"rows": {"set": "affected"}, "columns": ["churn", "coverage"], "sortBy": "churn", "dock": "right", "title": "Affected"}}"#;
    let layer: LayerSpec = serde_json::from_str(json).unwrap();
    let rt = serde_json::to_string(&layer).unwrap();
    let back: LayerSpec = serde_json::from_str(&rt).unwrap();
    assert_eq!(layer, back);
    if let LayerSpec::Panel(p) = &layer {
        assert!(matches!(&p.rows, PanelRows::Set(SetRef::Name(s)) if s == "affected"));
        assert!(matches!(&p.sort_by, Some(SortKey::Metric(MetricRef(s))) if s == "churn"));
        assert_eq!(p.dock, Dock::Right);
    } else {
        panic!("expected Panel");
    }
}

#[test]
fn panel_builtin_sort_round_trips() {
    let json = r#"{"panel": {"id": "__palette", "rows": {"set": "__palette"}, "sortBy": "nameLength", "limit": 12, "dock": "float", "title": "File"}}"#;
    let layer: LayerSpec = serde_json::from_str(json).unwrap();
    let rt = serde_json::to_string(&layer).unwrap();
    let back: LayerSpec = serde_json::from_str(&rt).unwrap();
    assert_eq!(layer, back);
    if let LayerSpec::Panel(p) = &layer {
        assert!(matches!(p.sort_by, Some(SortKey::Builtin(BuiltinSort::NameLength))));
        assert_eq!(p.limit, Some(12));
    } else {
        panic!("expected Panel");
    }
}

#[test]
fn panel_edge_groups_round_trips() {
    let json = r#"{"panel": {"id": "__callers", "rows": {"edgeGroups": {"of": {"ids": ["$focus"]}, "relation": "calls", "direction": "in"}}, "dock": "left", "title": "Callers"}}"#;
    let layer: LayerSpec = serde_json::from_str(json).unwrap();
    let rt = serde_json::to_string(&layer).unwrap();
    let back: LayerSpec = serde_json::from_str(&rt).unwrap();
    assert_eq!(layer, back);
    if let LayerSpec::Panel(p) = &layer {
        if let PanelRows::EdgeGroups { relation, direction, .. } = &p.rows {
            assert_eq!(relation, "calls");
            assert_eq!(*direction, EdgeDirection::In);
        } else {
            panic!("expected EdgeGroups");
        }
    } else {
        panic!("expected Panel");
    }
}

#[test]
fn full_spec_from_json_document_parses() {
    let json = r#"{
        "outriderView": 1,
        "meta": { "title": "Churn overview" },
        "space": { "kind": "treemap" },
        "sets": {
            "focusSet": { "ids": ["$focus"] },
            "neighborSet": { "neighbors": "focus" }
        },
        "layers": [
            { "fill": { "metric": "churn", "channel": "stripe", "scale": "percentile" } },
            { "marks": { "on": "focusSet", "kind": "focusRing" } },
            { "marks": { "on": "neighborSet", "kind": "neighbor" } }
        ]
    }"#;
    let spec: ViewSpec = serde_json::from_str(json).unwrap();
    assert_eq!(spec.version, 1);
    assert_eq!(spec.sets.len(), 2);
    assert_eq!(spec.layers.len(), 3);
}
