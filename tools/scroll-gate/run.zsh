#!/bin/zsh
set -euo pipefail
script_dir=${0:A:h}
exec python3 -I "$script_dir/driver.py" "$@"
