"""Draw `public/favicon.ico`.

Run BY HAND after editing, not from `build.rs`:

    python3 favicon.py

It needs Pillow, which CI does not have and should not have to — the icon changes once a year and the
result is committed. Keeping the generator beside it is the point: a favicon checked in without one is a
binary nobody can adjust, and the next person redraws it from scratch to move a line by a pixel.

TWO COMPOSITIONS, not one scaled. At 16px a checkbox beside a line is three pixels of box against six of
line and comes out as porridge — so 16 is a tick over two lines, and the larger sizes carry the checklist
the tick is standing in for. That is also why this writes the .ico container itself: PIL's ICO writer takes
one image and resizes it for every size asked for, which would put the blurred checklist right back.

The blue is `--accent` from `css/01-tokens.css`. If that token moves, this moves with it.
"""

import struct
from io import BytesIO

from PIL import Image, ImageDraw

OUT = "public/favicon.ico"

BLUE = (13, 110, 253, 255)      # --accent, the product's own blue
WHITE = (255, 255, 255, 255)
FADED = (255, 255, 255, 165)

S = 8  # supersample factor: everything is drawn big and shrunk with antialiasing


def canvas(size):
    img = Image.new("RGBA", (size * S, size * S), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    radius = int(size * 0.22 * S)
    draw.rounded_rectangle([0, 0, size * S - 1, size * S - 1], radius=radius, fill=BLUE)
    return img, draw


def tick(draw, x, y, w, h, width, color=WHITE):
    """A checkmark as two strokes, so it keeps its shape at any size."""
    draw.line([(x, y + h * 0.55), (x + w * 0.38, y + h)], fill=color, width=width, joint="curve")
    draw.line([(x + w * 0.38, y + h), (x + w, y)], fill=color, width=width, joint="curve")


def big(size):
    """Three rows: a checked box, then two unchecked ones with their lines."""
    img, draw = canvas(size)
    u = size * S / 48.0  # design grid is 48 units

    rows = [
        (12.0, WHITE, 30.0, True),
        (24.0, FADED, 26.0, False),
        (36.0, FADED, 21.0, False),
    ]

    for cy, color, bar_len, checked in rows:
        box = 11.0
        bx0, by0 = 7.0 * u, (cy - box / 2) * u
        bx1, by1 = (7.0 + box) * u, (cy + box / 2) * u
        stroke = max(1, int(2.0 * u))

        if checked:
            draw.rounded_rectangle([bx0, by0, bx1, by1], radius=2.5 * u, fill=WHITE)
            tick(
                draw,
                bx0 + 2.6 * u,
                by0 + 3.0 * u,
                6.0 * u,
                4.6 * u,
                max(1, int(1.8 * u)),
                color=BLUE,
            )
        else:
            draw.rounded_rectangle(
                [bx0, by0, bx1, by1], radius=2.5 * u, outline=color, width=stroke
            )

        # The line beside the box: the task itself.
        lx0 = 22.0 * u
        lx1 = (22.0 + bar_len) * u
        draw.rounded_rectangle(
            [lx0, (cy - 1.6) * u, lx1, (cy + 1.6) * u], radius=1.6 * u, fill=color
        )

    return img.resize((size, size), Image.LANCZOS)


def small(size):
    """16px: a tick over two bars. Anything with a box in it turns to porridge here."""
    img, draw = canvas(size)
    u = size * S / 16.0

    tick(draw, 3.2 * u, 3.0 * u, 9.6 * u, 5.4 * u, max(1, int(2.2 * u)))

    for cy, length in ((11.6, 9.6), (14.0, 6.6)):
        draw.rounded_rectangle(
            [3.2 * u, (cy - 0.8) * u, (3.2 + length) * u, (cy + 0.8) * u],
            radius=0.8 * u,
            fill=WHITE if length > 8 else FADED,
        )

    return img.resize((size, size), Image.LANCZOS)


frames = [(16, small(16)), (32, big(32)), (48, big(48)), (64, big(64))]

payloads = []
for size, img in frames:
    buf = BytesIO()
    img.save(buf, format="PNG", optimize=True)
    payloads.append((size, buf.getvalue()))

offset = 6 + 16 * len(payloads)
directory = b""
blobs = b""

for size, data in payloads:
    directory += struct.pack(
        "<BBBBHHII",
        size if size < 256 else 0,  # width, 0 means 256
        size if size < 256 else 0,  # height
        0,  # palette count: 0 for a true-colour image
        0,  # reserved
        1,  # colour planes
        32,  # bits per pixel
        len(data),
        offset,
    )
    blobs += data
    offset += len(data)

ico = struct.pack("<HHH", 0, 1, len(payloads)) + directory + blobs

with open(OUT, "wb") as file:
    file.write(ico)

print(f"wrote {OUT}: {len(ico)} bytes, sizes {[size for size, _ in payloads]}")
