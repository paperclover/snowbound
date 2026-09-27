#!/bin/sh
# Installs Snowbound into ~/.local with a launcher entry; with a notebook folder, the
# launcher opens it.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
data=${XDG_DATA_HOME:-$HOME/.local/share}
bin=$HOME/.local/bin
mkdir -p "$bin" "$data/applications"
cp "$here/bin/snowbound" "$bin/snowbound"
cp -R "$here/share/icons" "$data/"
exec_line="$bin/snowbound"
if [ $# -gt 0 ]; then
    notebook=$(cd "$1" && pwd)
    # Desktop entries quote arguments in double quotes, escape \ " ` $ and double %.
    quoted=$(printf '%s' "$notebook" | sed -e 's/[\\"`$]/\\\\&/g' -e 's/%/%%/g')
    exec_line="$exec_line --notebook \"$quoted\""
fi
sed "s|^Exec=.*|Exec=$exec_line|" "$here/share/applications/snowbound.desktop" \
    > "$data/applications/snowbound.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q "$data/icons/hicolor" 2>/dev/null || true
echo "Installed $bin/snowbound and the Snowbound launcher."
