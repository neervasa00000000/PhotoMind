"""Generate a reproducible, isolated decoder/scan stress library (Pillow required).

Development only; never included in the application. Refuses an existing output.
"""
import argparse
import hashlib
import json
import random
import shutil
from pathlib import Path
from PIL import Image, ImageFilter

parser = argparse.ArgumentParser()
parser.add_argument("output", type=Path)
parser.add_argument("--count", type=int, default=1000)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=False)
rng = random.Random(20260918)
base = Image.frombytes("RGB", (160, 120), rng.randbytes(160 * 120 * 3))
for ext in ["jpg", "png", "webp", "tiff", "bmp"]:
    base.save(args.output / f"normal.{ext}")
base.resize((6000, 4000)).save(args.output / "large.jpg", quality=90)
base.filter(ImageFilter.GaussianBlur(5)).save(args.output / "blurred.jpg")
for name, value in [("black", 0), ("white", 255), ("underexposed", 8), ("overexposed", 245)]:
    Image.new("RGB", (160, 120), (value, value, value)).save(args.output / f"{name}.jpg")
shutil.copyfile(args.output / "normal.jpg", args.output / "distant-copy-99999.jpg")
shutil.copyfile(args.output / "normal.jpg", args.output / "jpeg-renamed.cr3")
(args.output / "zero.jpg").write_bytes(b"")
(args.output / "corrupt.png").write_bytes(b"\x89PNG\r\n\x1a\ninvalid")
(args.output / "truncated.nef").write_bytes(b"II\x2a\x00" + b"\x00" * 8)
(args.output / "bad.avif").write_bytes(b"\x00\x00\x00\x18ftypavif\x00\x00\x00\x00avifmif1" + b"\x00\x00\x00\x09meta\x00")
fixtures = Path(__file__).resolve().parents[1] / "src-tauri" / "fixtures"
for name in ["sample-canon-m200.cr3", "sample.nef", "sample-avif-8bpc-yuv420.avif", "sample.heic"]:
    if (fixtures / name).is_file():
        shutil.copyfile(fixtures / name, args.output / name)
for index in range(args.count):
    image = Image.frombytes("RGB", (160, 120), rng.randbytes(160 * 120 * 3))
    image.save(args.output / f"stress-{index:05}.jpg", quality=85)
manifest = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(args.output.iterdir()) if p.is_file()}
(args.output.parent / f"{args.output.name}-manifest.json").write_text(json.dumps(manifest, indent=2))
print(json.dumps({"folder": str(args.output), "files": len(manifest), "bytes": sum(p.stat().st_size for p in args.output.iterdir())}))
