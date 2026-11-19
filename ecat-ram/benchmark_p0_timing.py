import csv
import math
import time
from collections import Counter
from pathlib import Path

import pysoem

clock = time.perf_counter_ns
adapter = r"\Device\NPF_{9269FBF0-8A17-4A3B-A238-90DF8FACBB09}"
master = pysoem.Master()
master.open(adapter)
slave = None

def pct(samples, quantile):
    values = sorted(samples)
    return round(values[math.ceil(len(values) * quantile) - 1] / 1000, 2)

def cycle():
    start = clock()
    master.send_processdata()
    wkc = master.receive_processdata(2000)
    return start, clock() - start, wkc

try:
    if master.config_init() != 1:
        raise RuntimeError("Expected a single connected test slave")
    slave = master.slaves[0]
    if (slave.man, slave.id) != (0x24121, 1):
        raise RuntimeError("Unexpected slave identity")
    master.config_map()
    if len(slave.output) != 4 or len(slave.input) != 4:
        raise RuntimeError("Unexpected PDO mapping")
    master.state_check(pysoem.SAFEOP_STATE, 100000)
    slave.output = bytes(4)
    cycle()
    master.state = pysoem.OP_STATE
    master.write_state()
    for _ in range(300):
        cycle()
        master.read_state()
        if slave.state == pysoem.OP_STATE:
            break
        time.sleep(0.002)
    if slave.state != pysoem.OP_STATE:
        raise RuntimeError(f"OP not reached: state={slave.state:#x} AL={slave.al_status:#x}")
    expected = master.expected_wkc
    with Path(__file__).with_name("hpm_p0_timing.csv").open("w", newline="") as f:
        fields = ["period_us", "duration_s", "frames", "expected_wkc", "bad_wkc", "skipped_slots", "interval_p50_us", "interval_p99_us", "interval_max_us", "rtt_p50_us", "rtt_p99_us", "rtt_max_us", "state", "al_status", "wkc_histogram"]
        writer = csv.DictWriter(f, fields)
        writer.writeheader()
        for period_us in (2000, 1000, 500):
            period = period_us * 1000
            first = clock()
            end = first + 10_000_000_000
            due = first
            starts, rtt = [], []
            counts = Counter()
            skipped = 0
            while due < end:
                while clock() < due:
                    pass
                start, elapsed, wkc = cycle()
                starts.append(start)
                rtt.append(elapsed)
                counts[wkc] += 1
                due += period
                now = clock()
                if now >= due:
                    missed = min((now - due) // period + 1, max(0, (end - 1 - due) // period + 1))
                    skipped += missed
                    due += missed * period
            intervals = [b - a for a, b in zip(starts, starts[1:])]
            master.read_state()
            row = dict(zip(fields, [period_us, 10, len(starts), expected, len(starts)-counts[expected], skipped, pct(intervals, .5), pct(intervals, .99), pct(intervals, 1), pct(rtt, .5), pct(rtt, .99), pct(rtt, 1), hex(slave.state), hex(slave.al_status), dict(counts)]))
            writer.writerow(row)
            f.flush()
            print(row, flush=True)
            if slave.state != pysoem.OP_STATE:
                break
finally:
    if slave is not None:
        master.state = pysoem.SAFEOP_STATE
        master.write_state()
        slave.state_check(pysoem.SAFEOP_STATE, 100000)
        master.read_state()
        print(f"Final state={slave.state:#x}", flush=True)
    master.close()
