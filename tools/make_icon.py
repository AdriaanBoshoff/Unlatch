"""Generate Unlatch's icon set (assets/unlatch.ico + assets/unlatch.png).

Drawn procedurally at 1024px and downsampled, so the icon can be regenerated
or tweaked without any design tools:  python tools/make_icon.py
Requires Pillow.
"""
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

S = 1024  # master canvas size
ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "assets"

TOP = (99, 102, 241)      # indigo
BOTTOM = (20, 184, 166)   # teal
WHITE = (255, 255, 255, 255)


def gradient_tile() -> Image.Image:
    grad = Image.new("RGBA", (S, S))
    px = grad.load()
    for y in range(S):
        for x in range(S):
            t = min(1.0, max(0.0, (x * 0.35 + y * 0.65) / S))
            px[x, y] = tuple(int(TOP[i] + (BOTTOM[i] - TOP[i]) * t) for i in range(3)) + (255,)
    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).rounded_rectangle((40, 40, S - 40, S - 40), radius=220, fill=255)
    tile = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    tile.paste(grad, (0, 0), mask)
    return tile


def padlock() -> Image.Image:
    layer = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)

    # Body
    body = (262, 470, 762, 860)
    d.rounded_rectangle(body, radius=70, fill=WHITE)

    # Open shackle: an arch lifted up and swung right, so only its right leg
    # would re-enter the body. Drawn as a thick arc + one straight leg.
    w = 78
    cx, top = 600, 170
    r = 165
    arc_box = (cx - r, top, cx + r, top + 2 * r)
    d.arc(arc_box, start=180, end=360, fill=WHITE, width=w)
    # Right leg reaches down toward (but not into) the body: unlatched.
    d.rectangle((cx + r - w, top + r, cx + r, 430), fill=WHITE)
    # Left leg is short and floats free above the body.
    d.rectangle((cx - r, top + r, cx - r + w, 370), fill=WHITE)
    d.ellipse((cx - r, 370 - w // 2, cx - r + w, 370 + w // 2), fill=WHITE)

    # Keyhole punched through the body
    hole = Image.new("L", (S, S), 0)
    hd = ImageDraw.Draw(hole)
    hd.ellipse((462, 580, 562, 680), fill=255)
    hd.polygon([(480, 650), (544, 650), (566, 770), (458, 770)], fill=255)
    alpha = layer.getchannel("A")
    alpha.paste(0, (0, 0), hole)
    layer.putalpha(alpha)
    return layer


def build() -> Image.Image:
    tile = gradient_tile()
    lock = padlock()
    shadow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    shadow.paste((15, 23, 42, 90), (0, 18), lock.getchannel("A"))
    shadow = shadow.filter(ImageFilter.GaussianBlur(18))
    out = Image.alpha_composite(tile, shadow)
    return Image.alpha_composite(out, lock)


def main() -> None:
    ASSETS.mkdir(exist_ok=True)
    master = build()
    master.resize((256, 256), Image.LANCZOS).save(ASSETS / "unlatch.png")
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    master.resize((256, 256), Image.LANCZOS).save(
        ASSETS / "unlatch.ico", sizes=[(s, s) for s in sizes]
    )
    # Raw RGBA used by the app at runtime for the window icon and header logo
    # (avoids pulling an image decoder into the binary).
    (ASSETS / "unlatch_128.rgba").write_bytes(master.resize((128, 128), Image.LANCZOS).tobytes())
    print("wrote", ASSETS / "unlatch.ico", "and", ASSETS / "unlatch.png")


if __name__ == "__main__":
    main()
