# Header Stack Content Clipping

## Problem

Container headers render at a fixed screen-space height so they remain readable while zooming. The packed layout reserves header space in world units, so that reservation becomes progressively smaller on screen as the camera zooms out. Descendant nodes can consequently paint upward behind their ancestors' accumulated header rows.

## Desired Behavior

- Named container headers retain their current fixed screen-space height and stacking order.
- A descendant node must not paint above the bottom of its accumulated ancestor-header stack.
- A partially obscured descendant paints only in the remaining visible region below the stack.
- A descendant with no visible region below the stack does not paint.
- World-space layout, camera behavior, focus geometry, and hit-testing remain unchanged.

## Design

Extend the existing screen-space paint-item construction to carry the active ancestor header-stack bottom into descendant clipping. Intersect each descendant's projected paint bounds with a top edge at that stack bottom before producing its visual primitives. Keep header geometry separate: a container's own header still participates in the stack and paints at its pinned position.

Apply the same clipped top consistently to the node background and node-owned content so no texture, text, border, or selection fill protrudes through the headers. Do not reflow packed rectangles or modify camera coordinates; this is a paint-time visibility correction.

## Alternatives Rejected

- **Scale headers with zoom:** avoids overlap but makes labels unreadable at the zoom levels where pinned headers are most useful.
- **Reflow layout for each zoom:** reserves physical header space but makes geometry camera-dependent, destabilizing navigation, caching, and hit-testing.

## Verification

Add focused unit tests for nested fixed-height headers at low zoom:

- a descendant beginning above the stack is clipped to the stack bottom;
- a descendant entirely behind the stack is omitted;
- a descendant already below the stack is unchanged;
- nested headers continue to stack in order while their descendants are clipped.

No automated screenshot, end-to-end, or acceptance testing is required. Visual acceptance will be performed by the user.
