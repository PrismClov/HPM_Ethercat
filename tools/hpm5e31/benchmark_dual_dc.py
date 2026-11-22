"""Measure zero-output PDO delivery and DC offset on the Windows two-board bench."""
import argparse
from collections import Counter
import ctypes
import json
import math
import time

import pysoem


def percentile(values, fraction):
    if not values:
        return None
    return sorted(values)[math.ceil(len(values) * fraction) - 1]


def states(master):
    master.read_state()
    return [(s.state, s.al_status) for s in master.slaves]


def read_al(slave):
    try:
        data = slave._fprd(0x130, 6)
        return [int.from_bytes(data[:2], "little"), int.from_bytes(data[4:], "little")]
    except pysoem.WkcError:
        return None


def measure(master, hz, seconds):
    period = round(1e9 / hz)
    clock = time.perf_counter_ns
    slots = hz * seconds
    start = clock()
    slot = -5 * hz
    due = start
    measurement_start = None
    integral = 0
    previous = None
    next_diag = start + 100_000_000
    wkc = Counter()
    late, rtt, intervals, dc = [], [], [], []
    diag_fail = 0
    completion_overruns = 0
    settled_dc = []
    invalid_streak = 0
    fault_snapshot = None
    expected = master.expected_wkc
    while slot < slots:
        while clock() < due:
            pass
        sent = clock()
        missed = (sent - due) // period
        slot += missed
        due += missed * period
        if slot >= slots:
            break
        measuring = slot >= 0
        if measuring:
            if measurement_start is None:
                measurement_start = due - slot * period
                next_diag = sent
            late.append((sent - due) / 1000)
        master.send_processdata()
        remaining_us = (due + period - clock()) // 1000
        work = master.receive_processdata(max(1, remaining_us))
        received = clock()
        if measuring:
            rtt.append((received - sent) / 1000)
            wkc[work] += 1
        if measuring and received > due + period:
            completion_overruns += 1
        invalid_streak = invalid_streak + 1 if work != expected else 0
        if measuring and invalid_streak >= 16 and fault_snapshot is None:
            fault_snapshot = {
                "slot": slot,
                "wkc": work,
                "al": [read_al(s) for s in master.slaves],
            }
        if work == expected:
            if measuring and previous is not None:
                intervals.append(abs(received - previous - period) / 1000)
            previous = received
        else:
            previous = None
        # Center the arriving PDO between SYNC0 edges and track oscillator drift.
        correction = 0
        if work == expected:
            phase_error = master.dc_time % period - period // 2
            integral = max(-period, min(period, integral + (phase_error > 0) - (phase_error < 0)))
            correction = max(-period // 20, min(period // 20, -phase_error // 100 - integral // 20))
        due += period + correction
        slot += 1
        # Budget the optional register read against the next PDO deadline.
        if measuring and received >= next_diag and due - received > 300_000:
            next_diag = received + 100_000_000
            try:
                data = master.slaves[1]._fprd(0x092c, 4, 150)
                raw = int.from_bytes(data, 'little')
                magnitude = raw & 0x7fffffff
                dc.append(magnitude)
                if received - measurement_start >= max(0, seconds - 60) * 1_000_000_000:
                    settled_dc.append(-magnitude if raw >> 31 else magnitude)
            except pysoem.WkcError:
                diag_fail += 1
    elapsed_s = (clock() - (measurement_start or start)) / 1e9
    end_states = [read_al(s) for s in master.slaves]
    master.state = pysoem.SAFEOP_STATE
    master.write_state()
    return dict(hz=hz, seconds=seconds, elapsed_s=elapsed_s, expected=expected, slots=slots,
                skipped=slots - sum(wkc.values()), wkc=dict(wkc), end=end_states,
                launch_late_p99_us=percentile(late, 0.99), rtt_p99_us=percentile(rtt, 0.99),
                rtt_max_us=max(rtt, default=None), interval_error_p99_us=percentile(intervals, 0.99),
                late_completions=completion_overruns, fault_snapshot=fault_snapshot,
                dc_samples=len(dc), dc_misses=diag_fail, dc_p50_ns=percentile(dc, 0.5),
                dc_p99_ns=percentile(dc, 0.99), dc_max_ns=max(dc, default=None),
                tail_samples=len(settled_dc),
                tail_abs_p99_ns=percentile([abs(v) for v in settled_dc], 0.99),
                tail_abs_max_ns=max(map(abs, settled_dc), default=None),
                tail_peak_to_peak_ns=max(settled_dc) - min(settled_dc) if settled_dc else None)


def run(adapter, hz, seconds):
    master = pysoem.Master()
    master.open(adapter)
    armed = []
    try:
        master.manual_state_change = True
        count = master.config_init()
        identities = [(s.man, s.id) for s in master.slaves]
        if count != 2 or identities != [(0x1337, 0x4d2), (0x24121, 1)]:
            raise RuntimeError("Expected STM digital IO followed by HPM5E31")
        master.state = pysoem.PREOP_STATE
        master.write_state()
        if master.state_check(pysoem.PREOP_STATE, 500000) != pysoem.PREOP_STATE:
            raise RuntimeError(f"PRE-OP entry failed: {states(master)}")
        master.config_map()
        if master.expected_wkc != 6 or not master.config_dc():
            raise RuntimeError("Unexpected PDO layout or DC configuration")
        period = round(1e9 / hz)
        for slave in master.slaves:
            slave.output = bytes(len(slave.output))
            armed.append(slave)
            slave.dc_sync(True, period)
            cycle_ns = int.from_bytes(slave._fprd(0x9a0, 4), "little")
            activation = slave._fprd(0x981, 1)[0]
            if cycle_ns != period or activation & 3 != 3:
                raise RuntimeError("SYNC0 configuration did not read back")
        master.state = pysoem.SAFEOP_STATE
        master.write_state()
        if master.state_check(pysoem.SAFEOP_STATE, 500000) != pysoem.SAFEOP_STATE:
            raise RuntimeError(f"SAFE-OP entry failed: {states(master)}")
        modes = []
        for slave in armed:
            try:
                modes.append(int.from_bytes(slave.sdo_read(0x1c32, 1), "little"))
            except pysoem.SdoError:
                modes.append(None)
        print(json.dumps(dict(output_sync_modes=modes)), flush=True)
        # Drive the reference clock and PDOs while SYNC0 starts in SAFE-OP.
        due = time.perf_counter_ns()
        for _ in range(hz):
            while time.perf_counter_ns() < due:
                pass
            master.send_processdata()
            master.receive_processdata(2000)
            due = max(due + period, time.perf_counter_ns())
        master.state = pysoem.OP_STATE
        master.write_state()
        for _ in range(400):
            master.send_processdata()
            master.receive_processdata(2000)
            if states(master) == [(8, 0), (8, 0)]:
                break
            time.sleep(min(0.002, 1 / hz))
        else:
            raise RuntimeError(f"OP entry failed: {states(master)}")
        print(json.dumps(measure(master, hz, seconds)), flush=True)
    finally:
        try:
            if armed:
                master.state = pysoem.SAFEOP_STATE
                master.write_state()
                master.state_check(pysoem.SAFEOP_STATE, 200000)
                for slave in armed:
                    slave.dc_sync(False, 0)
                for slave in armed:
                    status = read_al(slave)
                    if status and status[0] == 0x14:
                        slave.state = pysoem.SAFEOP_STATE | pysoem.STATE_ACK
                        slave.write_state()
                        slave.state_check(pysoem.SAFEOP_STATE, 200000)
                final_states = [read_al(s) for s in armed]
                print(json.dumps(dict(cleanup=final_states)), flush=True)
                if final_states != [[4, 0], [4, 0]]:
                    raise RuntimeError(f"SAFE-OP cleanup incomplete: {final_states}")
        finally:
            master.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("interface")
    parser.add_argument("frequencies", nargs="+", type=int)
    parser.add_argument("--seconds", type=int, default=60)
    args = parser.parse_args()
    if args.seconds <= 0 or any(not 1 <= hz <= 10000 for hz in args.frequencies):
        parser.error("Duration must be positive and frequency in 1..10000 Hz")
    kernel = ctypes.windll.kernel32
    kernel.GetCurrentProcess.restype = ctypes.c_void_p
    kernel.SetPriorityClass.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
    if not kernel.SetPriorityClass(kernel.GetCurrentProcess(), 0x80):
        raise ctypes.WinError()
    for frequency in args.frequencies:
        run(args.interface, frequency, args.seconds)
