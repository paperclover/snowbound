#!/bin/sh
# Drives the Mac OS X 10.6 test machine over ssh (scp doesn't pass its proxy).
#
#   remote.sh run [-C DIR] BINARY [ARGS...]
#                                    ship to /tmp/snow-leopard, run in DIR (default there),
#                                    print any new crash log
#   remote.sh mirror PATH...         copy repo paths to the same absolute paths there, so
#                                    test binaries find fixtures by CARGO_MANIFEST_DIR
#   remote.sh pull REMOTE [LOCAL]    copy a file back (e.g. a screenshot)
#   remote.sh sdk                    fetch the machine's MacOSX10.6.sdk for link.sh
#   remote.sh sh COMMAND...          run a shell command there
set -eu
host=${SNOW_LEOPARD_HOST:-osx10.6@10.0.0.1}
here=$(cd "$(dirname "$0")" && pwd)
dir=/tmp/snow-leopard

case "${1:-}" in
run)
    shift
    cwd=$dir
    if [ "$1" = -C ]; then cwd=$2; shift 2; fi
    binary=$1
    shift
    name=$(basename "$binary")
    ssh "$host" "mkdir -p $dir && cat > $dir/$name && chmod +x $dir/$name" < "$binary"
    quoted=
    for arg; do quoted="$quoted '$(printf %s "$arg" | sed "s/'/'\\\\''/g")'"; done
    # A crash report lands a few seconds after the process dies.
    ssh "$host" "touch $dir/.start && cd '$cwd' && status=0 && $dir/$name$quoted || status=\$?
        if [ \$status -ge 128 ]; then
            sleep 5
            for log in \$(find ~/Library/Logs/CrashReporter -name '${name}_*' -newer $dir/.start); do
                echo \"--- \$log\" >&2; sed -n '1,/^Binary Images/p' \"\$log\" >&2
            done
        fi
        exit \$status" < /dev/null
    ;;
mirror)
    shift
    root=$(cd "$here/../.." && pwd)
    (cd "$root" && COPYFILE_DISABLE=1 tar czf - --no-mac-metadata --no-xattrs --no-acls --exclude 'corpus/private' --exclude target "$@") |
        ssh "$host" "mkdir -p '$root' && tar xzf - -C '$root'"
    ;;
pull)
    ssh "$host" "cat '$2'" > "${3:-$(basename "$2")}"
    ;;
sdk)
    mkdir -p "$here/../../target/snow-leopard"
    ssh "$host" 'cd /Developer/SDKs && tar czf - MacOSX10.6.sdk' |
        tar xzf - -C "$here/../../target/snow-leopard"
    ;;
sh)
    shift
    ssh "$host" "$@"
    ;;
*)
    sed -n '2,12s/^# \{0,1\}//p' "$0" >&2
    exit 2
    ;;
esac
