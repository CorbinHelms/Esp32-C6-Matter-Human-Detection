#!/usr/bin/env python3
"""Passive, reset-surviving serial capture for USB-Serial-JTAG consoles.

Opens the port non-blocking (never waits on modem carrier — plain `stty`/`cat`
was observed hanging forever in open() on the C6's cdc-acm), puts it in raw
mode without touching DTR/RTS semantics, and appends everything to a log.
When the device re-enumerates (board reset — the crash case this exists for),
the read loop exits cleanly and the outer loop in the caller re-attaches, so
the ROM boot banner with the reset reason (`rst:0x...`) is captured.

Usage: serial_capture.py /dev/ttyACM1 /path/to/log
Runs until the device disappears (exit 0) or SIGTERM.
"""

import os
import select
import sys
import termios
import time


def main():
    dev, logpath = sys.argv[1], sys.argv[2]
    fd = os.open(dev, os.O_RDONLY | os.O_NOCTTY | os.O_NONBLOCK)
    try:
        attrs = termios.tcgetattr(fd)
        # cfmakeraw equivalent; leave control-line behavior (HUPCL off so a
        # later close doesn't drop DTR and reset the board).
        attrs[0] = 0  # iflag
        attrs[1] = 0  # oflag
        attrs[3] = 0  # lflag
        attrs[2] |= termios.CLOCAL | termios.CREAD
        attrs[2] &= ~termios.HUPCL
        termios.tcsetattr(fd, termios.TCSANOW, attrs)
    except termios.error:
        pass  # some cdc-acm states reject tcsetattr; reading still works

    with open(logpath, "ab", buffering=0) as log:
        ts = time.strftime("%H:%M:%S")
        log.write(f"=== capture attached {ts} ({dev}) ===\n".encode())
        while True:
            r, _, _ = select.select([fd], [], [], 1.0)
            if not r:
                continue
            try:
                chunk = os.read(fd, 4096)
            except OSError:
                break  # EIO: device re-enumerated (board reset)
            if chunk == b"":
                break
            log.write(chunk)
        ts = time.strftime("%H:%M:%S")
        log.write(f"\n=== device gone {ts} (reset?) ===\n".encode())


if __name__ == "__main__":
    main()
