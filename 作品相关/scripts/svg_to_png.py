#!/usr/bin/env python3
"""Convert generated SVG intermediates to PNG, then remove the intermediates."""
from pathlib import Path
import shutil
import subprocess

import cairosvg

ROOT = Path(__file__).resolve().parents[1]
FIGURES = ROOT / "figures"
FONT_FAMILY = "Noto Sans CJK SC"

# Fail before writing PNGs if the CJK font needed by the SVGs is unavailable.
if shutil.which("fc-match"):
    match = subprocess.run(["fc-match", "-f", "%{family}", FONT_FAMILY], capture_output=True, text=True, check=True).stdout
    if FONT_FAMILY not in match:
        raise SystemExit(f"Missing required font: {FONT_FAMILY}")

for source in sorted(FIGURES.glob("*.svg")):
    target = source.with_suffix(".png")
    cairosvg.svg2png(url=str(source), write_to=str(target), output_width=None, output_height=None)
    source.unlink()
    print(f"{source.name} -> {target.name} (SVG removed)")
