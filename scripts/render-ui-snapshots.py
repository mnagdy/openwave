#!/usr/bin/env python3
"""Render actual egui test paint meshes to PNG for offline visual review.

Requires numpy and Pillow. Input comes from the optional OPENWAVE_UI_SNAPSHOTS
environment variable when running gui::tests::render_all_pages_for_visual_review.
This software renderer is a layout review aid, not the native GPU renderer.
"""
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image


def render(path):
    data = json.loads(path.read_text())
    width, height = map(int, data["size"])
    canvas = np.ones((height, width, 4), dtype=np.float32)
    textures = {}
    for delta in data["textures"]:
        w, h = delta["size"]
        pixels = np.asarray(delta["pixels"], dtype=np.float32).reshape(h, w, 4) / 255
        if delta["pos"] is None:
            textures[delta["id"]] = pixels
        else:
            x, y = delta["pos"]
            textures[delta["id"]][y:y+h, x:x+w] = pixels
    for mesh in data["meshes"]:
        texture = textures[mesh["texture"]]
        th, tw = texture.shape[:2]
        vertices = np.asarray([[*v[:4], *v[4]] for v in mesh["vertices"]], dtype=np.float32)
        vertices[:, 4:] /= 255
        clip = mesh["clip"]
        for tri in np.asarray(mesh["indices"]).reshape(-1, 3):
            a, b, c = vertices[tri]
            left = max(0, int(np.floor(min(a[0], b[0], c[0]))), int(np.ceil(clip[0]-.5)))
            right = min(width, int(np.ceil(max(a[0], b[0], c[0]))), int(np.ceil(clip[2]-.5)))
            top = max(0, int(np.floor(min(a[1], b[1], c[1]))), int(np.ceil(clip[1]-.5)))
            bottom = min(height, int(np.ceil(max(a[1], b[1], c[1]))), int(np.ceil(clip[3]-.5)))
            if right <= left or bottom <= top:
                continue
            denominator = (b[1]-c[1])*(a[0]-c[0]) + (c[0]-b[0])*(a[1]-c[1])
            if abs(denominator) < 1e-8:
                continue
            y, x = np.mgrid[top:bottom, left:right].astype(np.float32)
            x += .5
            y += .5
            wa = ((b[1]-c[1])*(x-c[0])+(c[0]-b[0])*(y-c[1])) / denominator
            wb = ((c[1]-a[1])*(x-c[0])+(a[0]-c[0])*(y-c[1])) / denominator
            wc = 1-wa-wb
            mask = (wa >= -1e-5) & (wb >= -1e-5) & (wc >= -1e-5)
            if not mask.any():
                continue
            values = wa[mask, None]*a[2:] + wb[mask, None]*b[2:] + wc[mask, None]*c[2:]
            tx = np.clip(values[:, 0]*tw-.5, 0, tw-1)
            ty = np.clip(values[:, 1]*th-.5, 0, th-1)
            x0, y0 = tx.astype(int), ty.astype(int)
            x1, y1 = np.minimum(x0+1, tw-1), np.minimum(y0+1, th-1)
            dx, dy = (tx-x0)[:, None], (ty-y0)[:, None]
            sample = (texture[y0,x0]*(1-dx)+texture[y0,x1]*dx)*(1-dy) + (texture[y1,x0]*(1-dx)+texture[y1,x1]*dx)*dy
            source = values[:, 2:]*sample
            region = canvas[top:bottom, left:right]
            region[mask] = source + region[mask]*(1-source[:, 3:4])
    output = path.with_suffix(".png")
    Image.fromarray(np.clip(canvas*255, 0, 255).astype(np.uint8)).save(output)
    print(output.name)


if __name__ == "__main__":
    for filename in sorted(Path(sys.argv[1]).glob("*.json")):
        render(filename)
