use outrider_view::spec::*;
use outrider_view::validate;

#[test]
fn structural_mark_without_basis_rejected() {
    let spec = ViewSpec {
        layers: vec![LayerSpec::Marks(MarksSpec {
            on: MarkTarget::Set(SetRef::Name("x".to_string())),
            kind: MarkKind::Hotspot,
            label: None,
            basis: None,
        })],
        sets: [("x".to_string(), SetExpr::Ids(vec![]))].into(),
        ..ViewSpec::default()
    };
    let violations = validate::validate(&spec, &|_| true, &|_| true);
    assert!(violations.iter().any(|v| v.rule == "structural_needs_basis"));
}

#[test]
fn agent_note_without_text_rejected() {
    let spec = ViewSpec {
        layers: vec![LayerSpec::Notes(NotesSpec(vec![NoteSpec {
            at: NoteAnchor::Live(LiveAnchor::Focus),
            source: NoteSource::Agent,
            text: None,
            pin: None,
            readout: None,
        }]))],
        ..ViewSpec::default()
    };
    let violations = validate::validate(&spec, &|_| true, &|_| true);
    assert!(violations.iter().any(|v| v.rule == "agent-note-needs-text"));
}

#[test]
fn unknown_metric_rejected() {
    let spec = ViewSpec {
        layers: vec![LayerSpec::Fill(FillSpec {
            metric: "bogus".to_string(),
            ..Default::default()
        })],
        ..ViewSpec::default()
    };
    let violations = validate::validate(&spec, &|m| m != "bogus", &|_| true);
    assert!(violations.iter().any(|v| v.rule == "unknown_metric"));
}

#[test]
fn unknown_set_reference_rejected() {
    let spec = ViewSpec {
        layers: vec![LayerSpec::Marks(MarksSpec {
            on: MarkTarget::Set(SetRef::Name("doesNotExist".to_string())),
            kind: MarkKind::FocusRing,
            label: None,
            basis: None,
        })],
        ..ViewSpec::default()
    };
    let violations = validate::validate(&spec, &|_| true, &|_| true);
    assert!(violations.iter().any(|v| v.rule == "unknown_set"));
}

#[test]
fn camera_step_out_of_bounds_rejected() {
    let spec = ViewSpec {
        camera: CameraSpec {
            step: Some(5),
            steps: vec![],
            ..CameraSpec::default()
        },
        ..ViewSpec::default()
    };
    let violations = validate::validate(&spec, &|_| true, &|_| true);
    assert!(
        violations.iter().any(|v| v.rule == "camera.step_bounds"),
        "expected camera.step_bounds violation"
    );
}

#[test]
fn space_gap_out_of_range_rejected() {
    let spec = ViewSpec {
        space: SpaceSpec {
            pack: Some(PackOverrides {
                gap: Some(100.0),
                max_display_lines: None,
            }),
            ..SpaceSpec::default()
        },
        ..ViewSpec::default()
    };
    let violations = validate::validate(&spec, &|_| true, &|_| true);
    assert!(violations.iter().any(|v| v.rule == "space.pack.gap"));
}

#[test]
fn space_max_display_lines_zero_rejected() {
    let spec = ViewSpec {
        space: SpaceSpec {
            pack: Some(PackOverrides {
                gap: None,
                max_display_lines: Some(0),
            }),
            ..SpaceSpec::default()
        },
        ..ViewSpec::default()
    };
    let violations = validate::validate(&spec, &|_| true, &|_| true);
    assert!(violations
        .iter()
        .any(|v| v.rule == "space.pack.max_display_lines"));
}

#[test]
fn unsupported_space_kind_soft_warning() {
    let spec = ViewSpec {
        space: SpaceSpec {
            kind: SpaceKind::Matrix,
            ..SpaceSpec::default()
        },
        ..ViewSpec::default()
    };
    let violations = validate::validate(&spec, &|_| true, &|_| true);
    let v = violations
        .iter()
        .find(|v| v.rule == "space.kind.unsupported");
    assert!(v.is_some(), "expected unsupported space kind warning");
    assert!(!v.unwrap().hard, "unsupported space kind should be soft");
}

#[test]
fn valid_spec_has_no_hard_violations() {
    let spec = ViewSpec {
        sets: [("focusSet".to_string(), SetExpr::Ids(vec!["$focus".to_string()]))].into(),
        layers: vec![
            LayerSpec::Fill(FillSpec {
                metric: "churn".to_string(),
                channel: FillChannel::Stripe,
                scale: Scale::Percentile,
                domain: None,
                ramp: None,
            }),
            LayerSpec::Marks(MarksSpec {
                on: MarkTarget::Set(SetRef::Name("focusSet".to_string())),
                kind: MarkKind::FocusRing,
                label: None,
                basis: None,
            }),
        ],
        ..ViewSpec::default()
    };
    let violations = validate::validate(&spec, &|m| m == "churn", &|_| true);
    assert!(violations.iter().all(|v| !v.hard));
}
