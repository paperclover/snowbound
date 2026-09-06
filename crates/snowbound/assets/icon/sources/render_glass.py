from pathlib import Path
import subprocess

root = Path(__file__).resolve().parent.parent
(root / 'exports').mkdir(exist_ok=True)
tool = '/Applications/Xcode.app/Contents/Applications/Icon Composer.app/Contents/Executables/ictool'
for suffix, rendition in (('', 'Default'), ('-Dark', 'Dark'), ('-Mono', 'Mono')):
    subprocess.run([
        tool, str(root / 'sources/Snowbound-Tahoe.icon'), '--export-image',
        '--output-file', str(root / 'exports' / f'Snowbound-Tahoe{suffix}.png'),
        '--platform', 'macOS', '--rendition', rendition,
        '--width', '1024', '--height', '1024', '--scale', '1', '--light-angle', '-53',
    ], check=True)
