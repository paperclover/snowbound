#!/usr/bin/env python3
"""Lists what an executable imports that Windows 7 lacks, which stops it loading there:
`imports.py EXE DLLS`, DLLS a folder of Windows 7's System32 DLLs (copied from the lab VM),
reading both with llvm-mingw's llvm-readobj."""
import os
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
READOBJ = Path(os.environ.get('LLVM_MINGW', ROOT / 'target/windows/llvm-mingw')) / 'bin/llvm-readobj'


def readobj(flag, path):
    return subprocess.run([READOBJ, flag, path], capture_output=True, text=True, check=True).stdout


def imports(executable):
    """Each imported DLL, lowercased, with the names imported from it."""
    found, dll = {}, None
    for line in readobj('--coff-imports', executable).splitlines():
        if match := re.match(r'\s*Name: (\S+)', line):
            dll = found.setdefault(match[1].lower(), set())
        elif (match := re.match(r'\s*Symbol: (\S+)', line)) and dll is not None:
            dll.add(match[1])
    return found


def main():
    executable, folder = map(Path, sys.argv[1:3])
    dlls = {path.name.lower(): path for path in folder.iterdir()}
    missing = []
    for dll, names in sorted(imports(executable).items()):
        if dll not in dlls:
            missing.append(f'{dll} (no such DLL): {", ".join(sorted(names))}')
            continue
        exported = set(re.findall(r'Name: (\S+)', readobj('--coff-exports', dlls[dll])))
        missing += [f'{dll}!{name}' for name in sorted(names - exported)]
    print('\n'.join(missing) or 'Every import exists on Windows 7.')
    sys.exit(1 if missing else 0)


if __name__ == '__main__':
    main()
