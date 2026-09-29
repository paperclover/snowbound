#!/bin/sh
# Drives the Mac OS X 10.6 test machine over ssh (scp doesn't pass its proxy).
#
#   remote.sh run [-C DIR] BINARY [ARGS...]
#                                    ship to /tmp/snow-leopard, run in DIR (default there),
#                                    print any new crash log
#   remote.sh start BINARY [ARGS...] ship and leave running there, output in NAME.log;
#                                    BINARY may be an .app bundle
#   remote.sh stop NAME              end what start started (a bundle by its executable)
#   remote.sh screen LOCAL.png       wake the display and capture it (screencapture
#                                    writes nothing from ssh)
#   remote.sh input ARGS...          click|double X Y | drag X Y X2 Y2 | scroll X Y LINES
#                                    | type TEXT | key KEYCODE [command|shift|option|control]...
#   remote.sh selectors BINARY       selector-like names BINARY holds that nothing on
#                                    10.6 implements
#   remote.sh mirror PATH...         copy repo paths to the same absolute paths there, so
#                                    test binaries find fixtures by CARGO_MANIFEST_DIR
#   remote.sh pull REMOTE [LOCAL]    copy a file back (e.g. a screenshot)
#   remote.sh sdk                    fetch the machine's MacOSX10.6.sdk for link.sh
#   remote.sh sh COMMAND...          run a shell command there
set -eu
host=${SNOW_LEOPARD_HOST:-osx10.6@10.0.0.1}
here=$(cd "$(dirname "$0")" && pwd)
dir=/tmp/snow-leopard

ship() {
    base=$(basename "$1")
    if [ -d "$1" ]; then
        (cd "$(dirname "$1")" && COPYFILE_DISABLE=1 tar czf - --no-mac-metadata --no-xattrs "$base") |
            ssh "$host" "mkdir -p $dir && rm -rf '$dir/${base:?}' && tar xzf - -C $dir"
    else
        ssh "$host" "mkdir -p $dir && cat > $dir/$base && chmod +x $dir/$base" < "$1"
    fi
}

# Builds a console/ tool against the 10.6 SDK and ships it.
tool() {
    sdk=$here/../../target/snow-leopard/MacOSX10.6.sdk
    out=$here/../../target/snow-leopard/$1
    if [ ! "$out" -nt "$here/console/$1.c" ]; then
        clang -arch x86_64 -isysroot "$sdk" -mmacosx-version-min=10.6 -O2 -Wall \
            -framework ApplicationServices -framework CoreServices -lobjc \
            "$here/console/$1.c" -o "$out" 2>&1 | grep -v -e "MH_DYLIB_STUB" -e "no platform load" >&2 || true
    fi
    ship "$out"
}

quote() {
    quoted=
    for arg; do quoted="$quoted '$(printf %s "$arg" | sed "s/'/'\\\\''/g")'"; done
}

case "${1:-}" in
run)
    shift
    cwd=$dir
    if [ "$1" = -C ]; then cwd=$2; shift 2; fi
    binary=$1
    shift
    name=$(basename "$binary")
    ship "$binary"
    quote "$@"
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
start)
    shift
    name=$(basename "$1")
    program=./$name
    if [ -d "$1" ]; then
        name=$(/usr/libexec/PlistBuddy -c 'Print CFBundleExecutable' "$1/Contents/Info.plist")
        program=./$(basename "$1")/Contents/MacOS/$name
    fi
    ship "$1"
    shift
    quote "$@"
    ssh "$host" "cd $dir && (nohup $program$quoted < /dev/null > $name.log 2>&1 &)"
    ;;
stop)
    ssh "$host" "kill \$(ps axo pid,command | awk '{ sub(\".*/\", \"\", \$2) } \$2 == \"$2\" { print \$1 }')"
    ;;
screen)
    tool screen
    ssh "$host" "$dir/screen $dir/screen.png && cat $dir/screen.png" > "$2"
    ;;
input)
    shift
    tool input
    quote "$@"
    ssh "$host" "$dir/input$quoted"
    ;;
selectors)
    tool selectors
    strings -a "$2" | grep -E '^[a-z][a-z]+([A-Z][a-z0-9]+)*[A-Z]?[A-Za-z0-9]*(:([A-Za-z][A-Za-z0-9]*:)*)?$' |
        grep -E '[a-z]{2}[A-Z][a-z]|:$' | sort -u | ssh "$host" "$dir/selectors 2>/dev/null"
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
    sed -n '2,21s/^# \{0,1\}//p' "$0" >&2
    exit 2
    ;;
esac
