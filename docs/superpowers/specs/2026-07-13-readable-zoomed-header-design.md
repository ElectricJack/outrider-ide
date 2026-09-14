# Readable Zoomed Header Design

## Goal

Keep every rendered treemap container header readable while zooming out by preserving at least one screen-space line for the container name.

## Behavior

- Card, Detail, and Full container headers use the greater of their current zoom-scaled height and the natural one-line header height (`HEADER`).
- The existing 12 px header-name font remains unchanged.
- Metadata and summary rows remain optional. They are clipped as zoom reduces the available header height, while the name row remains visible.
- Label-tier containers retain their existing single centered name.
- Dot-tier and merged nodes remain text-free so distant views do not become an unreadable field of overlapping labels.
- A clamped header must remain within the container because every rung that paints a header is already at least one line tall.

## Consistency

One pure header-height calculation will be shared by paint preparation and pinned-ancestor stack prediction. Camera framing, pinned-header placement, background painting, and text placement therefore agree on the same one-line minimum.

## Testing

- Add a regression proving zoom-scaled container headers never fall below `HEADER`.
- Cover the transition around the one-line clamp and normal zoom where the existing multi-line height is preserved.
- Update pinned-stack coverage to prove its predicted height uses the same minimum.
- Run the focused treemap tests, then workspace formatting, strict Clippy, and all workspace tests.
