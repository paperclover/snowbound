#!/usr/bin/env python3
"""Extract a native capture with links for identical files."""
import hashlib
import os
from pathlib import Path, PurePosixPath
import sys
import zipfile

archive, destination = map(Path, sys.argv[1:])
destination.mkdir(parents=True, exist_ok=False)
seen = {}
with zipfile.ZipFile(archive) as captured:
    for entry in captured.infolist():
        relative = PurePosixPath(entry.filename.replace('\\', '/'))
        if relative.is_absolute() or '..' in relative.parts:
            raise ValueError('Archive path escapes the corpus directory')
        target = destination.joinpath(*relative.parts)
        if entry.filename.endswith(('/', '\\')):
            target.mkdir(parents=True, exist_ok=True)
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        data = captured.read(entry)
        digest = hashlib.sha256(data).digest()
        if digest in seen:
            target.symlink_to(os.path.relpath(seen[digest], target.parent))
        else:
            target.write_bytes(data)
            target.chmod(0o444)
            seen[digest] = target
print(f'Imported {len(seen)} distinct files into {destination}')
