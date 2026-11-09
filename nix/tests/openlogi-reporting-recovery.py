#!/usr/bin/env python3
"""Verify capture recovery against a running agent and a real HID++ receiver.

The test temporarily clears gesture diversion, then waits for the agent to
restore it. Run with the mouse awake and release its buttons during the test.
Only the gesture control's temporary diversion/raw-XY flags are changed; the
original values are restored even when recovery fails.
"""

import argparse
import os
import select
import time


class Receiver:
    def __init__(self, path, device_index):
        self.fd = os.open(path, os.O_RDWR | os.O_NONBLOCK)
        self.device_index = device_index

    def call(self, feature, function, payload):
        # Reserve software ID 15 for this isolated hardware test. The agent's
        # capture and inventory channels normally use IDs 1 and 2.
        header = bytes([0x11, self.device_index, feature, (function << 4) | 15])
        os.write(self.fd, header + payload.ljust(16, b"\0"))
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            if not select.select([self.fd], [], [], 0.1)[0]:
                continue
            report = os.read(self.fd, 64)
            if report[:4] == header:
                return report[4:]
            if (
                len(report) >= 7
                and report[1] == self.device_index
                and report[2] == 0xFF
                and report[4:6] == header[2:4]
            ):
                raise RuntimeError(f"HID++ error: {report.hex(' ')}")
        raise TimeoutError(f"No HID++ reply to {header.hex(' ')}")

    def reporting(self, feature):
        return self.call(feature, 2, bytes([0, 0xC3, 0]))[2] & 0x11

    def set_reporting(self, feature, flags):
        # Logitech 0x1b04 setCidReporting: bits 1 and 5 mark the temporary
        # diversion and raw-XY fields valid; bits 0 and 4 carry their values.
        self.call(feature, 3, bytes([0, 0xC3, 0x22 | flags]))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("hidraw", help="Receiver HID++ node, e.g. /dev/hidraw0")
    parser.add_argument("--device-index", type=int, default=1)
    parser.add_argument("--timeout", type=float, default=20)
    args = parser.parse_args()
    receiver = Receiver(args.hidraw, args.device_index)
    original = None
    try:
        feature = receiver.call(0, 0, bytes([0x1B, 0x04, 0]))[0]
        if feature == 0:
            raise RuntimeError("Device has no 0x1b04 controls feature")
        original = receiver.reporting(feature)
        if original != 0x11:
            raise RuntimeError(
                f"Gesture capture is already unhealthy: reporting=0x{original:02x}"
            )
        receiver.set_reporting(feature, 0)
        if receiver.reporting(feature) != 0:
            raise RuntimeError("Could not simulate loss of gesture reporting")
        print("Cleared gesture reporting; waiting for agent recovery", flush=True)
        deadline = time.monotonic() + args.timeout
        while time.monotonic() < deadline:
            if receiver.reporting(feature) == 0x11:
                print("PASS: agent restored diverted=true and raw_xy=true")
                break
            time.sleep(0.25)
        else:
            raise TimeoutError("Agent did not restore gesture reporting")
    finally:
        try:
            if original is not None:
                receiver.set_reporting(feature, original)
        finally:
            os.close(receiver.fd)
