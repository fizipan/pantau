#!/bin/bash
# Usage: scripts/measure-webview.sh <pantau-pid>
#
# Prints `footprint` for Pantau and for every com.apple.WebKit.* process that belongs to it, then the
# total in MB.
#
# Attribution: WebKit helpers are XPC services whose parent is launchd (ppid 1), so ppid cannot say
# which app owns them, and other apps own com.apple.WebKit.* processes too. The script asks macOS
# which process each helper is "responsible" to (responsibility_get_pid_responsible_for_pid, the same
# call Activity Monitor uses) and keeps the helpers whose responsible pid is Pantau's.
# Start Pantau with `open` (not from a shell), otherwise Pantau inherits the shell's responsible pid
# and the match is meaningless.
# Processes that WebKit starts for a window Pantau destroyed can linger briefly; they stay counted
# until they exit, which is what we want to measure.
set -euo pipefail

PID="${1:?usage: $0 <pantau-pid>}"

responsible() { # pid -> responsible pid, or -1
  python3 -I -c '
import ctypes, sys
f = ctypes.CDLL(None).responsibility_get_pid_responsible_for_pid
f.argtypes = [ctypes.c_int]; f.restype = ctypes.c_int
print(f(int(sys.argv[1])))' "$1"
}

footprint_mb() { # pid -> footprint in MB (one decimal), empty if the process is gone
  footprint "$1" 2>/dev/null | awk '
    /Footprint:/ { for (i = 1; i <= NF; i++) if ($i == "Footprint:") { v = $(i+1); u = $(i+2) }
      if (u == "KB") v /= 1024; else if (u == "GB") v *= 1024; printf "%.1f", v; exit }'
}

total=0
report() { # label pid
  local mb
  mb=$(footprint_mb "$2")
  [ -n "$mb" ] || { echo "$1 pid=$2: gone"; return; }
  echo "$1 pid=$2 Footprint: ${mb} MB"
  total=$(awk -v a="$total" -v b="$mb" 'BEGIN { printf "%.1f", a + b }')
}

report pantau "$PID"
for p in $(ps -axo pid=,comm= | awk '/com\.apple\.WebKit\./ { print $1 }'); do
  [ "$(responsible "$p")" = "$PID" ] || continue
  report "$(ps -p "$p" -o comm= | sed 's|.*/||')" "$p"
done
echo "TOTAL: ${total} MB"
