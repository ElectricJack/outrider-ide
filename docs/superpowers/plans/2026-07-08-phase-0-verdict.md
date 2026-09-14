# Milestone 0 verdict — GPUI under WSLg

- Date: 2026-07-08
- GPUI rev: 029bf2f284b4e59f20175d78443e630468f3a3e5 (gpui 0.2.2)
- Vulkan device (from `vulkaninfo --summary`): `llvmpipe (LLVM 20.1.2, 256 bits)` — Mesa 25.2.8, CPU software rasterizer; no dzn/Dozen hardware driver present
- Checklist: render **pass** · resize **pass** (via Win+arrow snapping) · drag **pass** (via snapping/reposition) · clean exit **pass** (Alt+F4, exit 0)
- **Verdict: WSLg viable**
- Notes:
  - Rendering is CPU-rasterized (llvmpipe). Fine for the hello-world; watch treemap frame rates in later phases — if animation stutters, revisit getting the Dozen (D3D12 passthrough) driver working or fall back to Windows-native.
  - WSLg's compositor does not provide server-side decorations to GPUI, and GPUI at this rev draws no client-side chrome by default — the window is borderless. Window management works via Windows shortcuts (Win+arrows, Alt+F4). Later phases should not depend on a titlebar existing; Zed draws its own chrome in-app.
  - Non-fatal `libEGL` warnings on startup ("failed to get driver name for fd -1") — cosmetic under WSLg.
  - `libxkbcommon-x11-dev` was missing from the initial dep install; now installed (a temporary linker workaround was added and then removed in-branch).
