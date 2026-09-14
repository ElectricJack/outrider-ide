use outrider_view::{
    FillChannel, FillSpec, LayerSpec, LiveAnchor, MarkKind, MarkTarget, MarksSpec, NoteAnchor,
    NoteSource, NoteSpec, NotesSpec, Scale, SetExpr, SetRef, ViewSpec,
};

use crate::settings::Settings;

/// Reserved set names for session navigation chrome.
const FOCUS_SET: &str = "focusSet";
const NEIGHBORS_SET: &str = "neighbors";

/// Add the focus-ring and neighbor-ring marks (and the sets they read) to
/// `spec` unless it already declares a `focusRing` marks layer. Idempotent.
/// User-authored specs get selection feedback without having to know about
/// the session's navigation marks.
pub(crate) fn ensure_navigation_marks(spec: &mut ViewSpec) {
    let has_focus_ring = spec.layers.iter().any(|l| {
        matches!(l, LayerSpec::Marks(m) if m.kind == MarkKind::FocusRing)
    });
    if has_focus_ring {
        return;
    }
    spec.sets
        .entry(FOCUS_SET.into())
        .or_insert_with(|| SetExpr::Ids(vec!["$focus".into()]));
    spec.sets
        .entry(NEIGHBORS_SET.into())
        .or_insert_with(|| SetExpr::Neighbors("$focus".into()));
    spec.layers.push(LayerSpec::Marks(MarksSpec {
        on: MarkTarget::Set(SetRef::Name(FOCUS_SET.into())),
        kind: MarkKind::FocusRing,
        label: None,
        basis: None,
    }));
    spec.layers.push(LayerSpec::Marks(MarksSpec {
        on: MarkTarget::Set(SetRef::Name(NEIGHBORS_SET.into())),
        kind: MarkKind::Neighbor,
        label: None,
        basis: None,
    }));
}

pub(crate) fn default_view(settings: &Settings) -> ViewSpec {
    let mut spec = ViewSpec::default();

    if settings.show_churn {
        spec.layers.push(LayerSpec::Fill(FillSpec {
            metric: "churn".into(),
            channel: FillChannel::Stripe,
            scale: Scale::Percentile,
            domain: None,
            ramp: None,
        }));
    }

    ensure_navigation_marks(&mut spec);

    spec.layers.push(LayerSpec::Notes(NotesSpec(vec![
        NoteSpec {
            at: NoteAnchor::Live(LiveAnchor::Hover),
            source: NoteSource::Doc,
            text: None,
            pin: None,
            readout: None,
        },
        NoteSpec {
            at: NoteAnchor::Live(LiveAnchor::Focus),
            source: NoteSource::Doc,
            text: None,
            pin: None,
            readout: None,
        },
    ])));

    spec
}
