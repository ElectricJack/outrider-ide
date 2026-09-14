#!/usr/bin/env bash
set -euo pipefail

cat <<'EOF'
Windows build: run build-windows.ps1 directly from PowerShell on Windows.

  From PowerShell, either:

  1. Build from the WSL filesystem (works but slow I/O):
     cd \\wsl.localhost\Ubuntu\home\jkern\dev\outrider-ide
     .\scripts\build-windows.ps1 -Release

  2. Clone to a Windows drive for faster builds:
     git clone \\wsl.localhost\Ubuntu\home\jkern\dev\outrider-ide D:\dev\outrider-ide
     cd D:\dev\outrider-ide
     .\scripts\build-windows.ps1 -Release

  Prerequisites: Visual Studio Build Tools with "Desktop development with C++"
  (provides MSVC linker, Windows SDK, and fxc.exe shader compiler).
EOF
