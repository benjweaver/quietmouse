#!/bin/sh
# Makes udev forget the removed rule.
udevadm control --reload 2>/dev/null || true
exit 0
