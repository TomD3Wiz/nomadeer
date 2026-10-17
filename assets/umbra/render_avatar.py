"""Renders Umbra's portrait: a holographic Aura, half taken by Sleeper circuitry.

Usage: python3 assets/umbra/render_avatar.py assets/umbra/umbra-avatar.png 512
Needs numpy. The seed is fixed, so the same script renders the same portrait.
"""

import struct
import sys
import zlib

import numpy as np

OUT = sys.argv[1]
SIZE = int(sys.argv[2]) if len(sys.argv) > 2 else 512
SS = 2  # supersampling
N = SIZE * SS
U = N / 512.0  # design units are a 512 canvas

rng = np.random.default_rng(1427)

yy, xx = np.mgrid[0:N, 0:N].astype(np.float32)
X = xx / U
Y = yy / U

img = np.zeros((N, N, 3), np.float32)


def rgb(r, g, b):
    return np.array([r, g, b], np.float32) / 255.0


def smooth(e0, e1, x):
    t = np.clip((x - e0) / (e1 - e0), 0.0, 1.0)
    return t * t * (3 - 2 * t)


def add(layer_alpha, color):
    """Additive light."""
    global img
    img += layer_alpha[..., None] * color


def over(layer_alpha, color):
    global img
    a = np.clip(layer_alpha, 0, 1)[..., None]
    img = img * (1 - a) + a * color


CYAN = rgb(93, 242, 255)
DEEP_CYAN = rgb(30, 120, 150)
BRONZE = rgb(214, 148, 74)
AMBER = rgb(255, 176, 80)
HOT = rgb(255, 228, 170)
VOID = rgb(6, 17, 22)

def seg_dist(px, py, ax, ay, bx, by):
    vx, vy = bx - ax, by - ay
    wx, wy = px - ax, py - ay
    t = np.clip((wx * vx + wy * vy) / max(vx * vx + vy * vy, 1e-6), 0, 1)
    return np.hypot(wx - t * vx, wy - t * vy)


def polyline_dist(px, py, points):
    d = np.full(px.shape, 1e9, np.float32)
    for (ax, ay), (bx, by) in zip(points, points[1:]):
        d = np.minimum(d, seg_dist(px, py, ax, ay, bx, by))
    return d


def curve(f, x0, x1, n=24):
    xs = np.linspace(x0, x1, n)
    return [(x, f(x)) for x in xs]


# --- Background: the dark of Anoikis, a faint nebula and stars -----------------
cx, cy = 256.0, 246.0
r = np.hypot(X - cx, Y - cy)
over(np.ones_like(X), VOID)
add(0.22 * np.exp(-((r / 230.0) ** 2)), rgb(20, 60, 70))
# Bronze bleed from the infected side.
add(0.18 * np.exp(-(((X - 400) / 160) ** 2 + ((Y - 200) / 220) ** 2)), rgb(120, 60, 20))
# Stars.
for _ in range(90):
    sx, sy = rng.uniform(0, 512, 2)
    br = rng.uniform(0.2, 0.9)
    d2 = (X - sx) ** 2 + (Y - sy) ** 2
    add(br * np.exp(-d2 / 0.9), rgb(200, 230, 240))

# --- Halo: a Sleeper glyph ring behind her head -------------------------------
theta = np.arctan2(Y - cy, X - cx)
ring_r = 196.0
ring = np.exp(-(((r - ring_r) / 2.2) ** 2))
segments = 28
seg = (theta + np.pi) / (2 * np.pi) * segments
frac = seg - np.floor(seg)
seg_idx = np.floor(seg).astype(int)
gap = (frac > 0.08) & (frac < 0.92)
keep = rng.random(segments) > 0.18
ring_mask = ring * gap * keep[seg_idx % segments]
add(0.95 * ring_mask, BRONZE)
add(0.35 * np.exp(-(((r - ring_r) / 9.0) ** 2)) * gap * keep[seg_idx % segments], BRONZE)
# Glyph ticks on the ring.
ticks = 56
tseg = (theta + np.pi) / (2 * np.pi) * ticks
tfrac = np.abs(tseg - np.round(tseg))
tick_len = np.where((np.round(tseg).astype(int) % 4) == 0, 16.0, 7.0)
tick = (tfrac < 0.06) & (r > ring_r + 6) & (r < ring_r + 6 + tick_len)
add(0.7 * tick.astype(np.float32), BRONZE)
# Inner broken cyan ring: what is left of Aura's interface.
inner_r = 178.0
inner = np.exp(-(((r - inner_r) / 1.4) ** 2)) * ((np.sin(theta * 9 + 0.6) > -0.2).astype(np.float32))
inner *= smooth(60, -40, X - cx)  # fades out on the infected side
add(0.6 * inner, CYAN)

# --- Her shape: head, hair, neck and shoulders --------------------------------
hx, hy = 256.0, 236.0


def head_sdf(px, py):
    """Approximate signed distance to a tapered oval face."""
    dy = py - hy
    # Narrower towards the chin.
    rx = 92.0 - np.clip(dy, 0, None) * 0.20 + np.clip(-dy, 0, None) * 0.02
    ry = np.where(dy > 0, 128.0, 118.0)
    q = np.hypot((px - hx) / rx, dy / ry)
    return (q - 1.0) * np.minimum(rx, ry)


face_d = head_sdf(X, Y)
face = smooth(1.5, -1.5, face_d)

# Hair: swept back, larger silhouette behind the face, falling to the shoulders.
hair_d = np.hypot((X - hx) / 118.0, (Y - (hy - 22)) / 140.0) - 1.0
hair_fall = np.hypot((X - hx) / 128.0, (Y - (hy + 80)) / 130.0) - 1.0
hair_shape = np.minimum(hair_d, hair_fall)
hair = smooth(0.02, -0.02, hair_shape) * (1 - face)
hair *= smooth(470, 380, Y)

neck = smooth(1.5, -1.5, np.abs(X - hx) - (40 + np.clip(Y - 350, 0, None) * 0.15)) * smooth(320, 340, Y)
shoulder_d = np.hypot((X - hx) / 210.0, (Y - 520.0) / 110.0) - 1.0
shoulders = smooth(0.01, -0.01, shoulder_d)
body = np.clip(neck + shoulders, 0, 1) * (1 - face)

# Holographic fill: translucent cyan, brighter at the rims.
scan = 0.75 + 0.25 * np.sin(Y * U * np.pi / 2.0) ** 2
rim = 0.8 * np.exp(-((face_d / 3.2) ** 2))
inside_glow = face * (0.30 + 0.25 * smooth(-90, 0, face_d))
# Soft modelling: brighter forehead and cheekbones, darker below the brow.
model = (
    0.55
    + 0.25 * np.exp(-(((X - hx) / 60) ** 2 + ((Y - 170) / 40) ** 2))
    + 0.18 * np.exp(-(((np.abs(X - hx) - 52) / 26) ** 2 + ((Y - 268) / 22) ** 2))
    - 0.25 * np.exp(-(((np.abs(X - hx) - 46) / 30) ** 2 + ((Y - 232) / 14) ** 2))
)
add(inside_glow * model * scan, rgb(40, 150, 175))
# Where the Sleeper code has taken her: a ragged front down her face, darker beyond.
front = hx + 16 + 9 * np.sin(Y * 0.055 + 1.0) + 5 * np.sin(Y * 0.17)
taken = smooth(-3, 3, X - front)
over(0.72 * taken * face, rgb(22, 15, 9))
add(0.10 * taken * face * model, BRONZE)
add(0.9 * rim * (1 - taken), CYAN)
add(0.9 * rim * taken, BRONZE)
# The front itself flickers where the two meet.
add(0.35 * np.exp(-(((X - front) / 2.0) ** 2)) * face, AMBER)

add(hair * 0.16 * scan, DEEP_CYAN)
hair_edge = np.exp(-((hair_shape * 120 / 2.5) ** 2)) * (1 - face) * smooth(470, 380, Y)
add(0.40 * hair_edge, CYAN)

for k in range(9):
    off = -100 + k * 25
    side = -1 if off < 0 else 1
    strand = curve(lambda y, o=off: hx + o * (1 + (y - 120) / 260) + side * 6 * np.sin((y - 120) / 50), 110, 400, 30)
    strand = [(x, y) for (y, x) in strand]
    sd = polyline_dist(X, Y, strand)
    add(0.16 * np.exp(-((sd / 1.2) ** 2)) * hair * (1 - taken if side > 0 else 1), CYAN)

body_edge = np.exp(-((np.minimum(shoulder_d * 110, 1e3) / 3.0) ** 2)) * (1 - face)
add(body * 0.16 * scan, DEEP_CYAN)
add(0.5 * body_edge * smooth(500, 420, Y) * smooth(300, 380, Y), CYAN)
neck_edge = np.exp(-(((np.abs(X - hx) - (40 + np.clip(Y - 350, 0, None) * 0.15)) / 2.0) ** 2))
add(0.5 * neck_edge * smooth(355, 375, Y) * smooth(430, 400, Y) * (1 - face), CYAN)

# --- Features -----------------------------------------------------------------


EYE_Y = 236.0
EYE_DX = 42.0
re = hx + EYE_DX


def eye_shape(ex):
    """Almond eye, signed in design units."""
    dx = (X - ex) / 25.0
    dy = (Y - EYE_Y) / 9.0
    upper = dy + 1.0 * (1 - dx * dx)
    lower = -dy + 0.75 * (1 - dx * dx)
    return np.minimum(upper, lower) * (np.abs(dx) < 1)


# Brows.
for side in (-1, 1):
    brow = curve(lambda x, s=side: 212 - 9 * np.exp(-(((x - (hx + s * 44)) / 20) ** 2)) + 0.06 * abs(x - hx), hx + side * 18, hx + side * 72)
    d = polyline_dist(X, Y, brow)
    color = CYAN if side < 0 else BRONZE
    add(0.75 * np.exp(-((d / 1.6) ** 2)), color)

# Left eye: Aura, cyan and calm.
left = eye_shape(hx - EYE_DX)
left_edge = np.exp(-((left / 0.08) ** 2)) * (np.abs(X - (hx - EYE_DX)) < 25)
add(0.9 * left_edge, CYAN)
iris_l = np.hypot(X - (hx - EYE_DX), Y - (EYE_Y + 0.5))
add(smooth(0, 0.05, left) * (0.9 * smooth(8.5, 6.5, iris_l)), CYAN)
add(smooth(0, 0.05, left) * (0.9 * smooth(3.0, 1.5, iris_l)), rgb(230, 255, 255))
add(0.25 * np.exp(-((iris_l / 18) ** 2)), CYAN)

# Nose: a bridge line and the soft base.
nose = [(hx - 2, 246), (hx - 5, 285), (hx - 11, 300)]
add(0.5 * np.exp(-((polyline_dist(X, Y, nose) / 1.3) ** 2)) * smooth(250, 270, Y), CYAN)
nostril = curve(lambda x: 304 - 3 * np.cos((x - hx) / 12 * 1.5), hx - 13, hx + 13)
add(0.55 * np.exp(-((polyline_dist(X, Y, nostril) / 1.3) ** 2)), CYAN)

# Lips: closed, unsmiling.
lip_mid = curve(lambda x: 336 + 1.5 * np.cos((x - hx) / 28 * np.pi / 2) - 2.0 * np.exp(-(((x - hx) / 6) ** 2)), hx - 28, hx + 28)
add(0.85 * np.exp(-((polyline_dist(X, Y, lip_mid) / 1.4) ** 2)), CYAN)
upper_lip = curve(lambda x: 330 - 3.5 * np.exp(-(((np.abs(x - hx) - 9) / 7) ** 2)), hx - 26, hx + 26)
add(0.35 * np.exp(-((polyline_dist(X, Y, upper_lip) / 1.2) ** 2)), CYAN)
add(0.22 * np.exp(-(((X - hx) / 14) ** 2 + ((Y - 345) / 3.5) ** 2)), CYAN)

# Cheekbone and jaw contour hints.
for side in (-1, 1):
    cheek = curve(lambda y, s=side: hx + s * (78 - 0.0025 * (y - 250) ** 2), 250, 300)
    cheek = [(x, y) for (y, x) in [(p[0], p[1]) for p in cheek]]
    add(0.18 * np.exp(-((polyline_dist(X, Y, cheek) / 1.5) ** 2)), CYAN)

# --- Infection: Sleeper circuitry across her right side ------------------------
# Traces start at the right eye and spread in 45-degree steps; most stay on the
# right half, a few tendrils reach out past her face and across her midline.
segments_list = []
nodes = []
DIRS = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)]
for i in range(44):
    if i < 12:
        ang = rng.uniform(0, 2 * np.pi)
        x, y = re + np.cos(ang) * 26, EYE_Y + np.sin(ang) * 14
    else:
        x, y = rng.uniform(hx + 26, hx + 88), rng.uniform(130, 350)
    d = int(rng.integers(0, 8))
    for step in range(int(rng.integers(2, 7))):
        length = float(rng.uniform(8, 26))
        dx, dy = DIRS[d]
        nl = length / (np.hypot(dx, dy))
        nx, ny = x + dx * nl, y + dy * nl
        # Keep the spread mostly on her right; let a few cross the midline.
        if nx < hx - 30 or (nx < hx + 6 and rng.random() > 0.15):
            d = int(rng.choice([0, 1, 7]))
            continue
        if np.hypot(nx - cx, ny - cy) > 172 or nx > hx + 150:
            break
        segments_list.append((x, y, nx, ny))
        x, y = nx, ny
        if rng.random() < 0.35:
            nodes.append((x, y))
        d = (d + int(rng.choice([-1, 0, 0, 1]))) % 8
    nodes.append((x, y))

trace_d = np.full(X.shape, 1e9, np.float32)
for ax, ay, bx, by in segments_list:
    x0, x1 = int(max(min(ax, bx) - 12, 0) * U), int(min(max(ax, bx) + 12, 512) * U)
    y0, y1 = int(max(min(ay, by) - 12, 0) * U), int(min(max(ay, by) + 12, 512) * U)
    sub = seg_dist(X[y0:y1, x0:x1], Y[y0:y1, x0:x1], ax, ay, bx, by)
    trace_d[y0:y1, x0:x1] = np.minimum(trace_d[y0:y1, x0:x1], sub)

# Brighter on her face, dimmer where the traces leave it.
eye_clear = smooth(16, 26, np.hypot((X - re) / 1.6, Y - EYE_Y))
on_face = eye_clear * (0.35 + 0.65 * smooth(4, -4, face_d)) * smooth(hx - 24, hx + 14, X)
add(0.75 * smooth(1.0, 0.35, trace_d) * on_face, AMBER)
add(0.16 * np.exp(-((trace_d / 4.0) ** 2)) * on_face, BRONZE)

node_d = np.full(X.shape, 1e9, np.float32)
for nx_, ny_ in nodes:
    node_d = np.minimum(node_d, np.hypot(X - nx_, Y - ny_))
add(smooth(2.4, 1.6, node_d) * 0.8, HOT)
add(0.18 * np.exp(-((node_d / 5) ** 2)), AMBER)

# Plating: angular dark Sleeper panels over her right cheek and temple, edged in bronze.
def plate(points):
    d = polyline_dist(X, Y, points + [points[0]])
    # Inside test via even-odd ray cast.
    inside = np.zeros(X.shape, bool)
    for (ax, ay), (bx, by) in zip(points, points[1:] + [points[0]]):
        cond = ((ay > Y) != (by > Y)) & (X < (bx - ax) * (Y - ay) / (by - ay + 1e-6) + ax)
        inside ^= cond
    return inside.astype(np.float32), d


for poly in (
    [(hx + 62, 180), (hx + 86, 196), (hx + 92, 230), (hx + 74, 214)],
    [(hx + 60, 262), (hx + 84, 258), (hx + 76, 300), (hx + 58, 292)],
    [(hx + 30, 150), (hx + 58, 140), (hx + 72, 166), (hx + 44, 170)],
):
    inside, d = plate(poly)
    over(0.55 * inside, rgb(26, 18, 12))
    add(0.8 * np.exp(-((d / 1.1) ** 2)), BRONZE)

# Right eye: Sleeper, a hot amber core that burns past its lids.
re = hx + EYE_DX
right = eye_shape(re)
right_edge = np.exp(-((right / 0.08) ** 2)) * (np.abs(X - re) < 25)
add(0.9 * right_edge, BRONZE)
iris_r = np.hypot(X - re, Y - (EYE_Y + 0.5))
add(smooth(0, 0.05, right) * smooth(11, 6, iris_r), AMBER)
add(smooth(0, 0.05, right) * smooth(4.0, 1.5, iris_r) * 1.2, HOT)
add(0.40 * np.exp(-((iris_r / 14) ** 2)), AMBER)
add(0.18 * np.exp(-((iris_r / 40) ** 2)), BRONZE)
# A horizontal Sleeper slit of light through the eye.
add(0.55 * np.exp(-(((Y - EYE_Y) / 1.2) ** 2)) * np.exp(-(((X - re) / 46) ** 2)), AMBER)

# --- Finish: scanlines and vignette ------------------------------------------
scan_all = 0.92 + 0.08 * np.sin(Y * U * np.pi / 1.5) ** 2
img *= scan_all[..., None]
vignette = 1.0 - 0.55 * smooth(150, 300, r)
img *= vignette[..., None]

# Gentle tone map so the hot spots roll off instead of clipping flat.
img = 1.0 - np.exp(-img * 1.35)
img = np.clip(img / (1.0 - np.exp(-1.35)), 0, 1)

# Downsample.
img = img.reshape(SIZE, SS, SIZE, SS, 3).mean(axis=(1, 3))
data = (np.clip(img, 0, 1) * 255 + 0.5).astype(np.uint8)


def write_png(path, pixels):
    h, w, _ = pixels.shape
    # Up filter on every row: scanlines repeat vertically, so it packs well.
    rows = pixels.astype(np.int16)
    up = np.vstack([rows[:1], (rows[1:] - rows[:-1]) % 256]).astype(np.uint8)
    raw = b"".join((b"\x00" if row == 0 else b"\x02") + up[row].tobytes() for row in range(h))

    def chunk(kind, payload):
        return (
            struct.pack(">I", len(payload))
            + kind
            + payload
            + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF)
        )

    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)))
        f.write(chunk(b"IDAT", zlib.compress(raw, 9)))
        f.write(chunk(b"IEND", b""))


write_png(OUT, data)
print("wrote", OUT, data.shape)
