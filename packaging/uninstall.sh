#!/bin/sh
# Removes quietmouse installed by install.sh on macOS, Linux or Windows (from
# Git Bash), keeping your config.
#
#   curl -fsSL https://raw.githubusercontent.com/benjweaver/quietmouse/main/packaging/uninstall.sh | sh
#
# The removal itself lives in install.sh, next to the install it undoes.
set -eu
curl -fsSL https://raw.githubusercontent.com/benjweaver/quietmouse/main/packaging/install.sh | sh -s -- --uninstall
