"""Lab settings come from the environment or the repository's untracked `.env`."""
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def setting(name):
    path = ROOT / ".env"
    if path.is_file():
        for line in path.read_text().splitlines():
            line = line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            key, value = line.split("=", 1)
            os.environ.setdefault(key.strip(), value.strip().strip('"'))
    return os.environ.get(name)


def require(name):
    value = setting(name)
    if not value:
        raise SystemExit("Set %s in %s (see .env.example)" % (name, ROOT / ".env"))
    return value
