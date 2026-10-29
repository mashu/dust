#!/usr/bin/env python3
"""Scale assets/icon.png into the hicolor sizes GNOME actually looks up.

The shell resolves a launcher icon only from directories named in
hicolor's index.theme. A size that is not listed there — the Dioxus
fallback is 460 — never appears. Several of the listed directories are
Threshold, so they will not scale a 512 master down to a menu icon.

The web page carries the 32 and 64 px ones inline as its tab icon, so they
are written into index.html again too.
"""

import base64
import re
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
MASTER = ROOT / "assets" / "icon.png"
OUT = ROOT / "assets" / "linux"
PAGE = ROOT / "index.html"

# Every 1× apps directory in hicolor-icon-theme. The bundler files each PNG
# under hicolor/{width}x{width}/apps from the image's own pixel size.
SIZES = (16, 22, 24, 32, 36, 48, 64, 72, 96, 128, 192, 256, 512)


def main() -> None:
    master = Image.open(MASTER).convert("RGBA")
    if master.size != (512, 512):
        raise SystemExit(f"{MASTER} must be 512×512, got {master.size}")

    OUT.mkdir(parents=True, exist_ok=True)
    for size in SIZES:
        dest = OUT / f"{size}x{size}.png"
        master.resize((size, size), Image.Resampling.LANCZOS).save(dest, optimize=True)

    page = PAGE.read_text()
    for size in (32, 64):
        png = base64.b64encode((OUT / f"{size}x{size}.png").read_bytes()).decode()
        page, found = re.subn(
            rf'(sizes="{size}x{size}" href="data:image/png;base64,)[^"]*',
            lambda link: link.group(1) + png,
            page,
        )
        if found != 1:
            raise SystemExit(f"{PAGE} has no inline {size}×{size} icon")
    PAGE.write_text(page)


if __name__ == "__main__":
    main()
