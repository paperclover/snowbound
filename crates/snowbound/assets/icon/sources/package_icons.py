from io import BytesIO
from pathlib import Path
import subprocess
import tempfile
from PIL import Image, ImageCms

root = Path(__file__).resolve().parent.parent
exports = root / 'exports'
srgb = ImageCms.createProfile('sRGB')
profile = ImageCms.ImageCmsProfile(srgb).tobytes()
board = Image.new('RGBA', (1800, 1100), '#edf0f2')
board.paste('#25343e', (0, 550, 1800, 1100))
for col, name in enumerate(('SnowLeopard', 'Sequoia', 'Tahoe')):
    for dark in (False, True):
        suffix = '-Dark' if name == 'Tahoe' and dark else ''
        im = Image.open(exports / f'Snowbound-{name}{suffix}.png')
        art = im.convert('RGBA')
        if im.info.get('icc_profile'):
            art = ImageCms.profileToProfile(art, ImageCms.ImageCmsProfile(BytesIO(im.info['icc_profile'])), srgb, outputMode='RGBA')
        if name == 'Tahoe':
            tile = art.resize((824, 824), Image.Resampling.LANCZOS)
            art = Image.new('RGBA', (1024, 1024))
            art.alpha_composite(tile, (100, 100))
        x, y = col * 600, 550 if dark else 0
        board.alpha_composite(art.resize((480, 480), Image.Resampling.LANCZOS), (x + 60, y))
        for size, center in ((64, 180), (32, 310), (16, 420)):
            board.alpha_composite(art.resize((size, size), Image.Resampling.LANCZOS), (x + center - size // 2, y + 500 - size // 2))
        if not dark:
            with tempfile.TemporaryDirectory() as temp:
                folder = Path(temp) / f'Snowbound-{name}.iconset'
                folder.mkdir()
                pixels = {}
                for size in (16, 32, 128, 256, 512):
                    for scale in (1, 2):
                        n = size * scale
                        dest = folder / (f'icon_{size}x{size}' + ('@2x' if scale == 2 else '') + '.png')
                        if n in pixels:
                            dest.symlink_to(pixels[n].name)
                        else:
                            art.resize((n, n), Image.Resampling.LANCZOS).save(dest, icc_profile=profile)
                            pixels[n] = dest
                subprocess.run(['iconutil', '-c', 'icns', str(folder), '-o', str(exports / f'Snowbound-{name}.icns')], check=True)
board.convert('RGB').save(root / 'Snowbound-comparison.png', icc_profile=profile)
