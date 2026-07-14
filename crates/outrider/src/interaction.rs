//! Pure actions emitted by input handling and consumed by `TreemapView`.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InteractionAction {
    DismissNotification,
}
