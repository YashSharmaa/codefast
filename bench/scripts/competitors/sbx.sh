#!/bin/bash
# Run any command inside the competitor sandbox environment (empty env, sandbox HOME, sandbox PATH).
#   sbx.sh ripwire --version
#   sbx.sh /usr/bin/time -l gtags ...
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh" || exit 1
sbx "$@"
