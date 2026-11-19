import os
import struct
import time
from collections import Counter

import pysoem

adapter = r"\Device\NPF_{9269FBF0-8A17-4A3B-A238-90DF8FACBB09}"
master = pysoem.Master()
master.open(adapter)
slave = None
try:
    count = master.config_init()
    if count != 1:
        raise RuntimeError(f"Expected one slave, got {count}")
    slave = master.slaves[0]
    if (slave.man, slave.id) != (0x24121, 1):
        raise RuntimeError(f"Unexpected slave identity {slave.man:#x}/{slave.id:#x}")
    master.config_map()
    dc = os.environ.get("ECAT_DC") == "1"
    if dc and not master.config_dc():
        raise RuntimeError("No DC-capable slave")
    if dc:
        slave.dc_sync(True, 500_000, 0)
    if len(slave.input) != 4 or len(slave.output) != 4:
        raise RuntimeError(f"Unexpected PDO sizes {len(slave.input)}/{len(slave.output)}")
    master.state_check(pysoem.SAFEOP_STATE, 50000)
    print(f"{slave.name} expected WKC={master.expected_wkc}", flush=True)
    slave.output = bytes(4)
    master.send_processdata()
    master.receive_processdata(2000)
    master.state = pysoem.OP_STATE
    master.write_state()
    for _ in range(300):
        master.send_processdata()
        master.receive_processdata(2000)
        master.read_state()
        if slave.state == pysoem.OP_STATE:
            break
        time.sleep(0.005)
    print(f"State={slave.state:#x} AL status={slave.al_status:#x}", flush=True)
    if slave.state != pysoem.OP_STATE:
        raise RuntimeError("Slave did not reach OP")
    expected = master.expected_wkc
    for value in (0, 1, 0):
        slave.output = struct.pack("<I", value)
        wkc = Counter()
        inputs = Counter()
        for _ in range(30):
            master.send_processdata()
            w = master.receive_processdata(2000)
            wkc[w] += 1
            if w == expected:
                inputs[int.from_bytes(slave.input[2:4], "little")] += 1
            time.sleep(0.01)
        try:
            readback = int.from_bytes(slave.sdo_read(0x7010, 0), "little")
        except Exception as exc:
            readback = repr(exc)
        master.read_state()
        print(f"RxPDO={value} slave readback={readback} TxPDO={dict(inputs)} WKC={dict(wkc)} state={slave.state:#x} AL={slave.al_status:#x}", flush=True)
finally:
    if slave is not None:
        if len(slave.output) == 4:
            slave.output = bytes(4)
            for _ in range(5):
                master.send_processdata()
                master.receive_processdata(2000)
        master.state = pysoem.SAFEOP_STATE
        master.write_state()
        master.read_state()
        slave.state_check(pysoem.SAFEOP_STATE, 100000)
        master.read_state()
        print(f"Final state={slave.state:#x}", flush=True)
    if slave is not None and os.environ.get("ECAT_DC") == "1":
        slave.dc_sync(False, 0, 0)
    master.close()
