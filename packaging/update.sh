#!/bin/sh
# Updates quietmouse installed by install.sh on macOS, Linux or Windows (from
# Git Bash) to the latest release. quietmouse never goes online by itself, so
# this is how it gets new versions.
#
#   curl -fsSL https://raw.githubusercontent.com/benjweaver/quietmouse/main/packaging/update.sh | sh
#
# The update itself lives in install.sh, next to the install it repeats.
set -eu
curl -fsSL https://raw.githubusercontent.com/benjweaver/quietmouse/main/packaging/install.sh | sh -s -- --update
