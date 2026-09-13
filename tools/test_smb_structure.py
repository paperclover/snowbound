#!/usr/bin/env python3
"""Run the notebook-structure lane against a disposable Linux lab VM's Samba share."""
import argparse
import os
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent / 'w7'))
from linux_vm import load_instance


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('vm', help='An already running, caller-owned Linux lab VM')
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    config = load_instance(args.vm)
    env = dict(os.environ, ONESTORE_SMB_LAB=f'127.0.0.1:{config["samba_port"]}')
    with (output / 'structure.log').open('w') as result:
        subprocess.run(['cargo', 'test', '-p', 'notebook', '--features', 'smb', 'smb::tests::live_structure', '--', '--ignored', '--exact'],
                       env=env, stdout=result, stderr=result, check=True, timeout=600)
    print('structure: passed', flush=True)


if __name__ == '__main__':
    main()
