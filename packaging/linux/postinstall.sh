#!/bin/sh
# Applies the udev rule and loads uinput straight away, so quietmouse works
# without a reboot or replugging. Harmless where udev isn't running, such as in
# containers.
udevadm control --reload 2>/dev/null || true
udevadm trigger --subsystem-match=hidraw --subsystem-match=misc 2>/dev/null || true
modprobe uinput 2>/dev/null || true
exit 0
