"""Measure output-to-input PDO echo on the HPM diagnostic firmware."""

import argparse
import csv
import math
import os
import select
import subprocess
import sys
import threading
import time


def snapshots(process):
    buffer = b""
    frame = {}
    while True:
        while b"\n" not in buffer:
            if os.name != "nt" and not select.select([process.stdout], [], [], 20)[0]:
                raise RuntimeError("EtherCAT helper response timed out")
            data = os.read(process.stdout.fileno(), 8192)
            if not data:
                raise RuntimeError("EtherCAT helper exited before the next snapshot")
            buffer += data
        line, buffer = buffer.split(b"\n", 1)
        fields = line.rstrip(b"\r").decode().split("\t")
        if fields[0] != "ecat":
            continue
        kind, *values = fields[1:]
        if kind == "error":
            raise RuntimeError(values[0])
        if kind == "slave":
            frame["op"] = int(values[4]) & 0x1f == 8
        elif kind == "pdo":
            frame["raw"] = int(values[2], 16) if values[2] else None
        elif kind == "echo":
            frame["token"], frame["tx"], frame["rx"] = map(int, values)
        elif kind == "end":
            yield frame
            frame = {}


def stats(samples):
    if not samples:
        return [None] * 4
    values = sorted(samples)
    return [round(values[math.ceil(len(values) * p) - 1] / 1000, 3) for p in (.5, .95, .99, 1)]


def measure(helper, index, seconds, period_us):
    process = subprocess.Popen(
        [helper, index, "--pdo-echo"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, bufsize=0
    )
    period = period_us * 1000
    clock = time.monotonic_ns
    watchdog = threading.Timer(seconds + 45, process.kill)
    watchdog.start()
    try:
        responses = snapshots(process)

        def sample(status=False):
            process.stdin.write(b"S\n" if status else b"P\n")
            return next(responses)

        deadline = clock() + 20_000_000_000
        while clock() < deadline:
            ready = sample(True)
            if ready["token"] and ready.get("op"):
                break
            time.sleep(.002)
        else:
            raise RuntimeError("No OP sample with the echo firmware signature")

        start = clock()
        slots = (seconds * 1_000_000 + period_us - 1) // period_us
        slot = sent = invalid = not_op = 0
        expected = {}
        arrival_times, latencies = [], []
        status_at = start

        def collect(frame):
            raw = frame.get("raw")
            if raw is None:
                return
            timestamp = expected.pop(raw >> 2, None)
            if timestamp is not None:
                latency = frame["rx"] - timestamp
                if latency < 0:
                    raise RuntimeError("Helper monotonic timestamps are inconsistent")
                latencies.append(latency)

        def wait_until(due):
            if period > 2_000_000:
                sleep = due - clock() - 200_000
                if sleep > 0:
                    time.sleep(sleep / 1e9)
            while clock() < due:
                pass

        while slot < slots:
            due = start + slot * period
            wait_until(due)
            now = clock()
            check = now >= status_at
            if check:
                status_at = now + 250_000_000
            frame = sample(check)
            if not frame["token"] or not frame["tx"] or not frame["rx"]:
                raise RuntimeError("Invalid echo timestamps or sequence")
            expected[frame["token"]] = frame["tx"]
            collect(frame)
            sent += 1
            invalid += frame.get("raw") is None
            not_op += not frame.get("op", False)
            arrival_times.append(frame["rx"])
            slot = max(slot + 1, (clock() - start) // period + 1)

        # Allow in-flight tokens to return without adding new tokens to the denominator.
        for _ in range(max(8, 20_000 // period_us)):
            wait_until(clock() + period)
            collect(sample())
            if not expected:
                break
        intervals = [b - a for a, b in zip(arrival_times, arrival_times[1:])]
        jitter = [abs(delta - period) for delta in intervals]
        return [
            period_us, seconds, slots, sent, len(latencies), not_op,
            100 * invalid / sent, 100 * (slots - sent) / slots, 100 * len(expected) / sent,
            *stats(intervals), *stats(jitter), *stats(latencies),
        ]
    finally:
        watchdog.cancel()
        process.stdin.close()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("helper")
    parser.add_argument("target", help="IgH master index or SOEM interface")
    parser.add_argument("seconds", type=int)
    parser.add_argument("periods", type=int, nargs="+")
    args = parser.parse_args()
    if not 1 <= args.seconds <= 600 or any(not 100 <= p <= 20000 for p in args.periods):
        parser.error("Duration must be 1–600 s, period 100–20000 µs")
    if sys.platform == "linux" and "microsoft" in os.uname().release.lower():
        parser.error("Use a native Linux or Windows host for this direct-link benchmark")
    header = ["target_us", "duration_s", "slots", "sent", "echoed", "not_op",
              "invalid_pdo_pct", "missed_slots_pct", "missing_echo_pct"]
    for metric in ("interval", "period_error", "latency"):
        header.extend(f"{metric}_{p}_us" for p in ("p50", "p95", "p99", "max"))
    writer = csv.writer(sys.stdout)
    writer.writerow(header)
    for period in args.periods:
        writer.writerow(measure(args.helper, args.target, args.seconds, period))
        sys.stdout.flush()


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError) as error:
        sys.exit(str(error))
