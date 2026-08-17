use outrider_view::{
    FillChannel, FillSpec, LayerSpec, LiveAnchor, MarkKind, MarkTarget, MarksSpec, NoteAnchor,
    NoteSource, NoteSpec, NotesSpec, Scale, SetExpr, SetRef, ViewSpec,
};

use crate::settings::Settings;

pub(crate) fn default_view(settings: &Settings) -> ViewSpec {
    let mut spec = ViewSpec::default();

    spec.sets
        .insert("focusSet".into(), SetExpr::Ids(vec!["$focus".into()]));
    spec.sets.insert(
        "neighbors".into(),
        SetExpr::Neighbors("$focus".into()),
    );

    if settings.show_churn {
        spec.layers.push(LayerSpec::Fill(FillSpec {
            metric: "churn".into(),
            channel: FillChannel::Stripe,
            scale: Scale::Percentile,
            domain: None,
            ramp: None,
        }));
    }

    spec.layers.push(LayerSpec::Marks(MarksSpec {
        on: MarkTarget::Set(SetRef::Name("focusSet".into())),
        kind: MarkKind::FocusRing,
        label: None,
        basis: None,
    }));
    spec.layers.push(LayerSpec::Marks(MarksSpec {
        on: MarkTarget::Set(SetRef::Name("neighbors".into())),
        kind: MarkKind::Neighbor,
        label: None,
        basis: None,
    }));

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
