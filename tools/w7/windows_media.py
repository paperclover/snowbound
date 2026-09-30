#!/usr/bin/env python3
"""Build Windows 10/11 installation ISOs from Microsoft's Media Creation Tool catalog."""

import argparse
import hashlib
from pathlib import Path
import shutil
import subprocess
import tempfile
import urllib.request
import xml.etree.ElementTree as ET

from env import require

# The catalogs the Media Creation Tool itself downloads.
CATALOGS = {
    "win10": ("https://go.microsoft.com/fwlink/?LinkId=841361", "x64"),
    "win11": ("https://go.microsoft.com/fwlink/?linkid=2156292", "ARM64"),
}


def tool(name):
    path = shutil.which(name) or "/opt/homebrew/bin/" + name
    if not Path(path).is_file():
        raise SystemExit("Install with: /opt/homebrew/bin/brew install wimlib xorriso")
    return path


def catalog_entry(base, work):
    url, arch = CATALOGS[base]
    cab = work / "products.cab"
    urllib.request.urlretrieve(url, cab)
    subprocess.run(["bsdtar", "-xf", str(cab), "-C", str(work), "products.xml"], check=True)
    for entry in ET.parse(work / "products.xml").getroot().iter("File"):
        field = lambda key: entry.findtext(key) or ""
        if (field("LanguageCode") == "en-us" and field("Architecture") == arch
                and "CLIENTCONSUMER_RET" in field("FileName")):
            return field("FilePath"), field("Sha1").lower()
    raise SystemExit("The %s catalog has no en-us %s consumer image" % (base, arch))


def download(url, sha1, path):
    digest = hashlib.sha1()
    with urllib.request.urlopen(url) as response, path.open("wb") as output:
        while chunk := response.read(8 * 1024 * 1024):
            output.write(chunk)
            digest.update(chunk)
    if digest.hexdigest() != sha1:
        raise SystemExit("Download failed SHA-1 verification: %s" % url)


def pro_index(esd):
    info = subprocess.check_output([tool("wimlib-imagex"), "info", str(esd)], text=True)
    index = None
    for line in info.splitlines():
        key, _, value = line.partition(":")
        if key.strip() == "Index":
            index = value.strip()
        elif key.strip() == "Edition ID" and value.strip() == "Professional":
            return index
    raise SystemExit("No Professional edition in %s" % esd)


def build(base):
    media = Path(require("ONE_VM_HOME")).expanduser() / "media"
    iso = media / ("%s.iso" % base)
    if iso.exists():
        raise SystemExit("Move the existing ISO first: %s" % iso)
    media.mkdir(parents=True, exist_ok=True)
    wim = tool("wimlib-imagex")
    with tempfile.TemporaryDirectory(prefix="one-media-", dir=media) as temporary:
        work = Path(temporary)
        url, sha1 = catalog_entry(base, work)
        esd = work / "image.esd"
        print("Downloading %s" % url, flush=True)
        download(url, sha1, esd)
        tree = work / "iso"
        sources = tree / "sources"
        subprocess.run([wim, "apply", str(esd), "1", str(tree)], check=True)
        subprocess.run([wim, "export", str(esd), "2", str(sources / "boot.wim"),
                        "--compress=LZX"], check=True)
        subprocess.run([wim, "export", str(esd), "3", str(sources / "boot.wim"),
                        "--boot"], check=True)
        # Solid LZMS keeps install.esd under the 4 GiB ISO 9660 file limit.
        subprocess.run([wim, "export", str(esd), pro_index(esd),
                        str(sources / "install.esd"), "--compress=LZMS", "--solid"],
                       check=True)
        esd.unlink()
        partial = work / "out.iso"
        # The no-prompt loader boots unattended instead of waiting for a key press.
        subprocess.run([
            tool("xorriso"), "-as", "mkisofs", "-quiet", "-iso-level", "3", "-J",
            "-joliet-long", "-V", base.upper(),
            "-e", "efi/microsoft/boot/efisys_noprompt.bin", "-no-emul-boot",
            "-o", str(partial), str(tree),
        ], check=True)
        partial.replace(iso)
    print(iso)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("base", choices=sorted(CATALOGS))
    build(parser.parse_args().base)


if __name__ == "__main__":
    main()
